/**
 * 数据质量 API
 *
 * 路由：
 * - GET /api/devices/point-quality       -> listPointQuality
 * - GET /api/devices/point-quality/stats -> getQualityStats
 *
 * 质量规则（QualityRule）落库到网关（/api/quality/rules），写成功后由网关安装到
 * 处理管道真实执行 range/rate/nan；后端不可用时回退 localStorage 并标记来源为 local。
 */
import { request } from './index'
import { scopedGet, scopedSet } from '../utils/gatewayStorage'
import { QUALITY_TEXT } from '../utils/qualityLabels'

export type Quality = 'good' | 'bad' | 'uncertain' | 'old'

/** 文案统一取自 utils/qualityLabels 单一来源，本页 key 域为小写 good/bad/uncertain/old */
export const QUALITY_LABEL: Record<Quality, string> = {
  good: QUALITY_TEXT.good, bad: QUALITY_TEXT.bad, uncertain: QUALITY_TEXT.uncertain, old: QUALITY_TEXT.old,
}

export const QUALITY_COLOR: Record<Quality, string> = {
  good: 'success', bad: 'danger', uncertain: 'warning', old: 'info',
}

export interface PointQuality {
  point_id: string
  point_name: string
  device_id: string
  device_name: string
  value: number | null
  quality: Quality
  consecutive_failures: number
  last_update_at: number
}

export interface QualityStats {
  good: number
  bad: number
  stale: number
  unknown: number
  total: number
}

export function listPointQuality(): Promise<PointQuality[]> {
  return request<PointQuality[]>({ url: '/api/devices/point-quality', method: 'GET' })
}

export function getQualityStats(): Promise<QualityStats> {
  return request<QualityStats>({ url: '/api/devices/point-quality/stats', method: 'GET' })
}

// ===== 采集退避状态（GET /api/devices/backoff，字段与 generated/PointBackoff.ts 保持一致）=====

export interface PointBackoff {
  point_id: string
  point_name: string
  device_id: string
  device_name: string
  bus: string
  /** 配置的基础采集间隔（毫秒） */
  base_interval_ms: number
  /** 当前实际采集间隔（退避时放大，毫秒） */
  current_interval_ms: number
  /** 退避倍率 = current / base（1 表示未退避） */
  backoff_multiplier: number
  /** 连续失败次数（>=3 进入退避） */
  consecutive_failures: number
  /** 连续重发次数 */
  stale_count: number
  last_good_value: number | null
  /** 上次成功时间戳（毫秒，0 表示从未成功） */
  last_good_ts: number
  quality: Quality
  /** 是否处于退避中 */
  backing_off: boolean
}

export function listPointBackoff(): Promise<PointBackoff[]> {
  return request<PointBackoff[]>({ url: '/api/devices/backoff', method: 'GET' })
}

// ===== 质量规则（远端优先 + 本地回退）=====

export interface QualityRule {
  id: string
  name: string
  condition: 'range' | 'rate' | 'stale' | 'nan'
  min?: number
  max?: number
  maxRate?: number
  staleSeconds?: number
  resultQuality: Quality
  enabled: boolean
}

/**
 * 规则数据来源：
 * - remote：已落库网关（写接口成功后规则会安装到处理管道，range/rate/nan 真实执行）
 * - local：后端不可用（离线 / 老版本）时的浏览器 localStorage 回退
 */
export type QualityRulesSource = 'remote' | 'local'

const RULES_KEY = 'data_quality_rules'
const RULES_URL = '/api/quality/rules'
const DEFAULT_RULES: QualityRule[] = [
  { id: 'q1', name: '超量程标记 Bad', condition: 'range', min: -100, max: 9999, resultQuality: 'bad', enabled: true },
  { id: 'q2', name: '突变标记 Uncertain', condition: 'rate', maxRate: 100, resultQuality: 'uncertain', enabled: true },
  { id: 'q3', name: '60秒未更新标记 Old', condition: 'stale', staleSeconds: 60, resultQuality: 'old', enabled: true },
]

/** 最近一次操作实际使用的数据来源（页面据此显示「远端已落库 / 仅本机」） */
let rulesSource: QualityRulesSource = 'local'

export function qualityRulesSource(): QualityRulesSource {
  return rulesSource
}

function loadRules(): QualityRule[] {
  try {
    // #6: 按激活网关隔离，避免多网关切换时质量规则串台
    const raw = scopedGet(RULES_KEY)
    return raw ? JSON.parse(raw) : DEFAULT_RULES
  } catch { return DEFAULT_RULES }
}
function saveRules(rules: QualityRule[]) {
  scopedSet(RULES_KEY, JSON.stringify(rules))
}

/** 读取远端规则；非数组响应视为失败，交给本地回退 */
async function fetchRemoteRules(): Promise<QualityRule[]> {
  const rules = await request<QualityRule[]>({ url: RULES_URL, method: 'GET' })
  if (!Array.isArray(rules)) throw new Error('invalid quality rules response')
  return rules
}

/**
 * 列出规则：优先读网关（已落库规则），失败时回退 localStorage。
 * 远端成功后把结果缓存到本地，供后续离线时作为「最近一次已知规则」。
 */
export async function listQualityRules(): Promise<QualityRule[]> {
  try {
    const rules = await fetchRemoteRules()
    rulesSource = 'remote'
    saveRules(rules)
    return rules
  } catch {
    rulesSource = 'local'
    return loadRules()
  }
}

export async function createQualityRule(rule: Omit<QualityRule, 'id'>): Promise<QualityRule> {
  try {
    const created = await request<QualityRule>({ url: RULES_URL, method: 'POST', data: rule })
    if (!created || typeof created.id !== 'string') throw new Error('invalid create response')
    rulesSource = 'remote'
    return created
  } catch {
    rulesSource = 'local'
    const rules = loadRules()
    const r = { ...rule, id: `q_${Date.now()}` }
    rules.push(r)
    saveRules(rules)
    return r
  }
}

export async function updateQualityRule(id: string, patch: Partial<QualityRule>): Promise<void> {
  try {
    // 后端 PUT 为局部更新：仅提交 patch，未提供字段保持网关侧原值
    await request<void>({ url: `${RULES_URL}/${id}`, method: 'PUT', data: patch })
    rulesSource = 'remote'
  } catch {
    rulesSource = 'local'
    const rules = loadRules()
    const r = rules.find((x) => x.id === id)
    if (r) Object.assign(r, patch)
    saveRules(rules)
  }
}

export async function deleteQualityRule(id: string): Promise<void> {
  try {
    await request<void>({ url: `${RULES_URL}/${id}`, method: 'DELETE' })
    rulesSource = 'remote'
  } catch {
    rulesSource = 'local'
    saveRules(loadRules().filter((x) => x.id !== id))
  }
}