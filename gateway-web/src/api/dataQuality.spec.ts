import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  listPointQuality,
  getQualityStats,
  listPointBackoff,
  listQualityRules,
  createQualityRule,
  updateQualityRule,
  deleteQualityRule,
  qualityRulesSource,
  QUALITY_LABEL,
  QUALITY_COLOR,
  type QualityRule,
} from './dataQuality'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

// 规则在远端（/api/quality/rules）落库；远端不可用时回退 localStorage 缓存。
// （scopedGet/scopedSet 由 utils/gatewayStorage 负责网关隔离，Web 模式下无激活网关 → 前缀为空
// → key 就是 'data_quality_rules'）
const RULES_KEY = 'data_quality_rules'
const RULES_URL = '/api/quality/rules'

function readStoredRules(): QualityRule[] {
  const raw = localStorage.getItem(RULES_KEY)
  return raw ? (JSON.parse(raw) as QualityRule[]) : []
}

describe('dataQuality 质量统计契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
    localStorage.clear()
  })

  // 锁定列表契约：GET /api/devices/point-quality
  it('点位质量列表走 GET /api/devices/point-quality，并原样透传后端数据', async () => {
    const rows = [
      {
        point_id: 'p1',
        point_name: 'PT01',
        device_id: 'dev-1',
        device_name: '温度计',
        value: 21.5,
        quality: 'good',
        consecutive_failures: 0,
        last_update_at: 1700000000000,
      },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listPointQuality()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/point-quality')
  })

  // 锁定统计契约：GET /api/devices/point-quality/stats（列表路径的子路径，不能写成 /api/devices/point-quality/summary）
  it('质量统计走 GET /api/devices/point-quality/stats', async () => {
    const stats = { good: 10, bad: 2, stale: 1, unknown: 3, total: 16 }
    requestMock.mockResolvedValue(stats)

    await expect(getQualityStats()).resolves.toEqual(stats)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/point-quality/stats')
  })

  // 锁定退避契约：GET /api/devices/backoff
  it('采集退避列表走 GET /api/devices/backoff', async () => {
    const rows = [
      {
        point_id: 'p1',
        point_name: 'PT01',
        device_id: 'dev-1',
        device_name: '温度计',
        bus: 'bus1',
        base_interval_ms: 1000,
        current_interval_ms: 4000,
        backoff_multiplier: 4,
        consecutive_failures: 3,
        stale_count: 2,
        last_good_value: 1.5,
        last_good_ts: 1700000000000,
        quality: 'bad',
        backing_off: true,
      },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listPointBackoff()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/backoff')
  })
})

describe('dataQuality 质量规则（远端优先）', () => {
  beforeEach(() => {
    requestMock.mockReset()
    localStorage.clear()
  })

  // 远端列表：GET /api/quality/rules 成功 → 来源标记 remote，并把结果缓存到本地
  it('列出规则优先走 GET /api/quality/rules，成功后标记来源为 remote', async () => {
    const remote: QualityRule[] = [
      { id: 'qr_1', name: '超量程', condition: 'range', min: -100, max: 9999, resultQuality: 'bad', enabled: true },
    ]
    requestMock.mockResolvedValue(remote)

    await expect(listQualityRules()).resolves.toEqual(remote)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe(RULES_URL)
    expect(qualityRulesSource()).toBe('remote')
    // 远端结果缓存到本地，供离线时使用
    expect(readStoredRules().map((r) => r.id)).toEqual(['qr_1'])
  })

  // 新增走 POST；后端返回落库后的规则（含 string id）
  it('新增规则优先走 POST /api/quality/rules', async () => {
    const created: QualityRule = {
      id: 'qr_abc', name: '自定义规则', condition: 'nan', resultQuality: 'bad', enabled: true,
    }
    requestMock.mockResolvedValue(created)

    await expect(
      createQualityRule({ name: '自定义规则', condition: 'nan', resultQuality: 'bad', enabled: true }),
    ).resolves.toEqual(created)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe(RULES_URL)
    expect(cfg.data).toMatchObject({ name: '自定义规则', condition: 'nan' })
    expect(qualityRulesSource()).toBe('remote')
  })

  // 更新走 PUT /api/quality/rules/:id，只提交补丁（后端局部更新）
  it('更新规则走 PUT /api/quality/rules/:id 并只提交补丁', async () => {
    requestMock.mockResolvedValue(undefined)

    await updateQualityRule('qr_1', { name: '新名', enabled: false })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe(`${RULES_URL}/qr_1`)
    expect(cfg.data).toEqual({ name: '新名', enabled: false })
    expect(qualityRulesSource()).toBe('remote')
  })

  // 删除走 DELETE /api/quality/rules/:id
  it('删除规则走 DELETE /api/quality/rules/:id', async () => {
    requestMock.mockResolvedValue(undefined)

    await deleteQualityRule('qr_1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe(`${RULES_URL}/qr_1`)
    expect(qualityRulesSource()).toBe('remote')
  })

  // 远端返回非数组视为失败 → 回退本地
  it('远端返回非数组时回退本地规则', async () => {
    requestMock.mockResolvedValue({ not: 'array' })

    const rules = await listQualityRules()

    expect(rules.map((r) => r.id)).toEqual(['q1', 'q2', 'q3'])
    expect(qualityRulesSource()).toBe('local')
  })
})

describe('dataQuality 质量规则（本地回退）', () => {
  beforeEach(() => {
    requestMock.mockReset()
    // 模拟后端不可用（离线 / 老版本网关）：所有请求都 reject
    requestMock.mockRejectedValue(new Error('offline'))
    localStorage.clear()
  })

  // 锁定默认规则契约：首次使用且远端不可用时返回内置 3 条规则
  // （超量程 Bad / 突变 Uncertain / 60s Old）
  it('远端不可用且未存储规则时返回内置默认 3 条并标记 local', async () => {
    const rules = await listQualityRules()

    expect(rules).toHaveLength(3)
    expect(rules.map((r) => r.id)).toEqual(['q1', 'q2', 'q3'])
    expect(rules.map((r) => r.condition)).toEqual(['range', 'rate', 'stale'])
    expect(rules.map((r) => r.resultQuality)).toEqual(['bad', 'uncertain', 'old'])
    expect(rules.every((r) => r.enabled)).toBe(true)
    expect(qualityRulesSource()).toBe('local')
  })

  // 锁定新增回退契约：自动生成 q_<时间戳> id，追加到已有规则之后并落盘到 localStorage
  // （预置已有规则，避免走「无存储 → 直接 push 到内置默认数组」这条分支）
  it('远端失败时新增规则生成 q_ 前缀 id 并持久化', async () => {
    localStorage.setItem(
      RULES_KEY,
      JSON.stringify([
        { id: 'q1', name: '超量程标记 Bad', condition: 'range', min: -100, max: 9999, resultQuality: 'bad', enabled: true },
      ]),
    )

    const created = await createQualityRule({
      name: '自定义规则',
      condition: 'nan',
      resultQuality: 'bad',
      enabled: true,
    })

    expect(created.id).toMatch(/^q_\d+$/)
    expect(created.name).toBe('自定义规则')
    // 已落盘，重读可见；追加在已有规则之后
    const stored = readStoredRules()
    expect(stored).toHaveLength(2)
    expect(stored[0]!.id).toBe('q1')
    expect(stored[1]!.id).toBe(created.id)
    expect(stored[1]!.name).toBe('自定义规则')
    expect(qualityRulesSource()).toBe('local')
  })

  // 锁定更新回退契约：按 id 打补丁（merge 语义），未命中的 id 不新增也不报错
  it('远端失败时更新规则按 id 合并补丁，未命中 id 时静默无操作', async () => {
    localStorage.setItem(
      RULES_KEY,
      JSON.stringify([
        { id: 'q1', name: '旧名', condition: 'range', min: 0, max: 10, resultQuality: 'bad', enabled: true },
      ]),
    )

    await updateQualityRule('q1', { name: '新名', enabled: false })

    const stored = readStoredRules()
    expect(stored).toHaveLength(1)
    expect(stored[0]!.name).toBe('新名')
    expect(stored[0]!.enabled).toBe(false)
    // 未提供的字段保持不变
    expect(stored[0]!.condition).toBe('range')
    expect(stored[0]!.max).toBe(10)

    await updateQualityRule('not-exist', { name: 'x' })
    expect(readStoredRules()).toHaveLength(1)
    expect(qualityRulesSource()).toBe('local')
  })

  // 锁定删除回退契约：只移除目标 id，其余保留
  it('远端失败时删除规则只移除目标 id', async () => {
    localStorage.setItem(
      RULES_KEY,
      JSON.stringify([
        { id: 'q1', name: 'a', condition: 'range', resultQuality: 'bad', enabled: true },
        { id: 'q2', name: 'b', condition: 'rate', resultQuality: 'uncertain', enabled: true },
      ]),
    )

    await deleteQualityRule('q1')

    const stored = readStoredRules()
    expect(stored.map((r) => r.id)).toEqual(['q2'])
    expect(qualityRulesSource()).toBe('local')
  })

  // 锁定容错契约：localStorage 内容损坏时回落默认规则，不抛异常
  it('存储内容损坏时回落默认规则', async () => {
    localStorage.setItem(RULES_KEY, '{not-json')

    const rules = await listQualityRules()

    expect(rules.map((r) => r.id)).toEqual(['q1', 'q2', 'q3'])
  })
})

describe('dataQuality 质量文案与配色常量', () => {
  // 锁定文案口径：与 utils/qualityLabels 单一来源一致（良好/坏值/不确定/过期）
  it('QUALITY_LABEL 与全站口径一致', () => {
    expect(QUALITY_LABEL.good).toBe('良好')
    expect(QUALITY_LABEL.bad).toBe('坏值')
    expect(QUALITY_LABEL.uncertain).toBe('不确定')
    expect(QUALITY_LABEL.old).toBe('过期')
  })

  // 锁定配色契约：good=success / bad=danger / uncertain=warning / old=info
  it('QUALITY_COLOR 映射固定', () => {
    expect(QUALITY_COLOR).toEqual({ good: 'success', bad: 'danger', uncertain: 'warning', old: 'info' })
  })
})
