import { qualityText } from '@/utils/qualityLabels'
import { qualityTagType as statusQualityTagType, type TagType } from '@/utils/statusType'
import { formatTimestamp } from '@/utils/datetime'

/**
 * 实时数据页的展示格式化纯函数集合。
 *
 * 从 `useRealtime` 拆出（W3b-13）：这些函数与 WS 连接、图表实例、轮询都无关，
 * 只做「值/时间/质量」到展示文案的映射，单独放这里才能被单测直接覆盖。
 */

/**
 * 坐标轴数值缩写：五位数以上直接标注会把左边距撑开、压扁绘图区。
 * 大于等于一万用「万」，一亿用「亿」，其余保留两位小数。
 */
export function compactAxis(v: number): string {
  if (!Number.isFinite(v)) return ''
  const abs = Math.abs(v)
  const trim = (n: number) => String(Number(n.toFixed(1)))
  if (abs >= 1e8) return `${trim(v / 1e8)}亿`
  if (abs >= 1e4) return `${trim(v / 1e4)}万`
  return Number.isInteger(v) ? String(v) : String(Number(v.toFixed(2)))
}

export function qualityClass(q: string) {
  if (q === 'good') return 'quality-good'
  if (q === 'stale') return 'quality-stale'
  if (q === 'uncertain') return 'quality-uncertain'
  return 'quality-bad'
}

/**
 * 质量 → 徽章颜色：统一委托全站本源 utils/statusType 的 qualityTagType。
 *
 * 实时页原有 info→danger 兜底会把「聚合/未知质量」误标成 danger（红色告警），
 * 与历史页口径冲突。已收敛为单一口径：聚合/未知落 info（中性），不再吓人地报红。
 * 这里保留同名导出仅为兼容既有 import。
 */
export function qualityTagType(q: string): TagType {
  return statusQualityTagType(q)
}

// 文案统一取自 utils/qualityLabels 单一来源（实时推送的 key 为小写 good/stale/bad/uncertain）
export const qualityLabel: Record<string, string> = {
  good: qualityText('good'),
  stale: qualityText('stale'),
  bad: qualityText('bad'),
  uncertain: qualityText('uncertain'),
}

/**
 * 数据类型 → 中文说明。
 *
 * 后端返回的是规范名（`PointDataType::canonical_name`）：u16/i16/u32/i32/u64/i64/f32/f64，
 * 历史配置与旧数据里还可能是 int16/uint16/int32/uint32/float/double 等别名（解析层仍接受），
 * 两套都登记，避免界面上把规范名显示成裸字符串。未登记的类型由调用方回退成原值。
 */
export const typeLabel: Record<string, string> = {
  u16: '无符号16位',
  i16: '有符号16位',
  u32: '无符号32位',
  i32: '有符号32位',
  u64: '无符号64位',
  i64: '有符号64位',
  f32: '单精度浮点',
  f64: '双精度浮点',
  // 历史别名
  uint16: '无符号16位',
  int16: '有符号16位',
  uint32: '无符号32位',
  int32: '有符号32位',
  float: '单精度浮点',
  double: '双精度浮点',
  bool: '布尔',
  string: '字符串',
}

export function formatValue(v: unknown): string {
  if (v == null) return '-'
  const n = typeof v === 'number' ? v : Number(v)
  if (!Number.isFinite(n)) {
    // 数字型 NaN / ±Infinity 不能直接显示成 "NaN"/"Infinity"——现场会误当成正常读数。
    // 但非数字字符串（"ON"、"OPEN" 等枚举原始值）必须原样保留，不能被 Number() 转坏。
    const isNonFiniteLiteral =
      typeof v === 'number' || /^(?:-?Infinity|NaN)$/.test(String(v).trim())
    return isNonFiniteLiteral ? '—' : String(v)
  }
  if (Number.isInteger(n)) return n.toString()
  return n.toFixed(3)
}

export function formatTime(ts: number): string {
  // 实时帧时间戳单位由后端决定，用 auto 阈值归一化（边界 1e12 当毫秒），style=time 只出时分秒
  return formatTimestamp(ts, { style: 'time', fallback: '-' })
}
