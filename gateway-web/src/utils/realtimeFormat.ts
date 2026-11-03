import { qualityText } from '@/utils/qualityLabels'
import { statusTagType, type TagType } from '@/utils/statusType'
import { formatTimeOfDay } from '@/utils/datetime'

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
 * 质量 → 徽章颜色：good/warn 类已登记在全站唯一映射表 utils/statusType；
 * 通用表落不到具体档位（未知质量）时按 danger 兜底，保持原有语义。
 */
export function qualityTagType(q: string): TagType {
  const t = statusTagType(q)
  return t === 'info' ? 'danger' : t
}

// 文案统一取自 utils/qualityLabels 单一来源（实时推送的 key 为小写 good/stale/bad/uncertain）
export const qualityLabel: Record<string, string> = {
  good: qualityText('good'),
  stale: qualityText('stale'),
  bad: qualityText('bad'),
  uncertain: qualityText('uncertain'),
}

export const typeLabel: Record<string, string> = {
  int16: '有符号16位',
  uint16: '无符号16位',
  int32: '有符号32位',
  uint32: '无符号32位',
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
  if (!ts) return '-'
  const ms = ts < 1e12 ? ts * 1000 : ts
  const d = new Date(ms)
  if (isNaN(d.getTime())) return '-'
  return formatTimeOfDay(d)
}
