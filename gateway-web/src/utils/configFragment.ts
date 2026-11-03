/**
 * 配置片段导入（合并语义）
 *
 * 与「导入导出」页的整份覆盖导入互补：这里只接受 6 类列表节（buses / devices /
 * points / device_groups / virtual_points / convert_rules），按主键 upsert 合并进
 * 当前配置，其余节（mqtt / web 等）原样保留，粘贴方不必提供完整配置。
 *
 * 字段规则不在前端维护：合并结果直接交给后端 `POST /api/config/validate`
 * （`GatewayConfig::validate_report` 为唯一来源），前端只负责「解析 + 合并 + 统计」。
 */
import type { GatewayConfig } from '@/api/types'

export const FRAGMENT_SECTIONS = [
  'buses',
  'devices',
  'points',
  'device_groups',
  'virtual_points',
  'convert_rules',
] as const

export type FragmentSection = (typeof FRAGMENT_SECTIONS)[number]

/** 片段：只允许出现列表节 */
export type ConfigFragment = Partial<Pick<GatewayConfig, FragmentSection>>

/** 节的中文名，页内表格与报错统一用这套 */
export const SECTION_LABELS: Record<FragmentSection, string> = {
  buses: '总线',
  devices: '设备',
  points: '点位',
  device_groups: '设备分组',
  virtual_points: '虚拟点位',
  convert_rules: '转换规则',
}

/**
 * 合并主键：buses 用 port_name（后端运行时也按它做 HashMap key），
 * devices 兼容旧配置没有 id 的情况回退 name，其余按 id。
 */
const KEY_HINT: Record<FragmentSection, string> = {
  buses: 'port_name',
  devices: 'id（或 name）',
  points: 'id',
  device_groups: 'id',
  virtual_points: 'id',
  convert_rules: 'id',
}

type Row = Record<string, unknown>

function keyOf(section: FragmentSection, item: Row): string {
  if (section === 'buses') return String(item.port_name ?? '').trim()
  if (section === 'devices') return String(item.id || item.name || '').trim()
  return String(item.id ?? '').trim()
}

export interface SectionStat {
  section: FragmentSection
  added: number
  updated: number
  unchanged: number
}

export type ParseFragmentResult = { ok: true; fragment: ConfigFragment } | { ok: false; error: string }

export function parseFragment(text: string): ParseFragmentResult {
  const trimmed = text.trim()
  if (!trimmed) return { ok: false, error: '请粘贴或上传要导入的配置片段' }

  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch (e) {
    return { ok: false, error: `JSON 解析失败：${e instanceof Error ? e.message : String(e)}` }
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return { ok: false, error: '片段必须是 JSON 对象，例如 {"points": [ ... ]}' }
  }

  const obj = parsed as Row
  const keys = Object.keys(obj)
  if (keys.length === 0) return { ok: false, error: '片段内容为空' }

  const unsupported = keys.filter((k) => !(FRAGMENT_SECTIONS as readonly string[]).includes(k))
  if (unsupported.length > 0) {
    return {
      ok: false,
      error: `不支持片段导入的配置节：${unsupported.join('、')}；这些节请用「导入导出」页的整份配置导入`,
    }
  }

  for (const section of keys as FragmentSection[]) {
    const value = obj[section]
    if (!Array.isArray(value)) {
      return { ok: false, error: `「${SECTION_LABELS[section]}」(${section}) 必须是数组` }
    }
    for (let i = 0; i < value.length; i++) {
      const item = value[i]
      if (typeof item !== 'object' || item === null || Array.isArray(item)) {
        return { ok: false, error: `「${SECTION_LABELS[section]}」第 ${i + 1} 条不是对象` }
      }
      if (!keyOf(section, item as Row)) {
        return {
          ok: false,
          error: `「${SECTION_LABELS[section]}」第 ${i + 1} 条缺少合并主键 ${KEY_HINT[section]}，无法判断是新增还是覆盖`,
        }
      }
    }
  }

  return { ok: true, fragment: obj as ConfigFragment }
}

/** JSON 比较（DeviceGroup.created_at 在 ts-rs 里是 bigint，不能直接 stringify） */
function fingerprint(value: unknown): string {
  return JSON.stringify(value, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))
}

/**
 * 把片段按主键 upsert 合并进当前配置。返回合并结果与每节的新增/覆盖/未变统计。
 * 未出现的节保持原样，因此贴一段点位不会碰 mqtt / web 等配置。
 */
export function mergeFragment(
  current: GatewayConfig,
  fragment: ConfigFragment,
): { merged: GatewayConfig; stats: SectionStat[] } {
  const merged = structuredClone(current) as unknown as Row
  const stats: SectionStat[] = []

  for (const section of FRAGMENT_SECTIONS) {
    const incoming = fragment[section]
    if (!incoming) continue

    const target = (merged[section] ?? []) as Row[]
    const index = new Map<string, number>()
    target.forEach((item, i) => index.set(keyOf(section, item), i))

    let added = 0
    let updated = 0
    let unchanged = 0
    for (const item of incoming as unknown as Row[]) {
      const key = keyOf(section, item)
      const at = index.get(key)
      if (at === undefined) {
        target.push(item)
        index.set(key, target.length - 1)
        added++
      } else if (fingerprint(target[at]) === fingerprint(item)) {
        unchanged++
      } else {
        target[at] = item
        updated++
      }
    }

    merged[section] = target
    stats.push({ section, added, updated, unchanged })
  }

  return { merged: merged as unknown as GatewayConfig, stats }
}

/**
 * 模板模式拦截：运行期点位由设备模板展开（expand_points），
 * `devices` 非空时 `cfg.points` 不参与采集，只导点位会「提示成功但不生效」。
 */
export function pointsIgnoredByTemplateMode(current: GatewayConfig, fragment: ConfigFragment): boolean {
  const hasDevices = (current.devices?.length ?? 0) > 0
  return hasDevices && (fragment.points?.length ?? 0) > 0
}
