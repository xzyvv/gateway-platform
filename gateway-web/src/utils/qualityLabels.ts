import { qualityTagType as statusQualityTagType, type TagType } from './statusType'

// 数据质量标签的单一来源：History / Realtime / DataQuality / DeviceShadow 等页面统一从这里取文案。
//
// 后端不同接口的 quality 值域不完全一致：
// - 实时推送（DataPoint.quality）：小写 good / bad / uncertain / stale
// - 历史查询（/api/data/history）：首字母大写 Good / Bad / Uncertain / Aggregated
// - 点位质量（/api/devices/point-quality）：小写 good / bad / uncertain / old
// 因此这里同时收录大小写 key，调用方无需关心来源接口的命名风格。

/** 统一质量文案：同一状态在所有页面措辞一致，避免用户在实时页看到「良好」、质量页看到「Good（正常）」 */
export const QUALITY_TEXT = {
  good: '良好',
  bad: '坏值',
  uncertain: '不确定',
  stale: '过期',
  old: '过期',
  aggregated: '聚合',
} as const

/** 历史接口使用首字母大写 key，单独保留一份视图供 qualityMap 复用 */
export const qualityMap: Record<string, string> = {
  Good: QUALITY_TEXT.good,
  Bad: QUALITY_TEXT.bad,
  Uncertain: QUALITY_TEXT.uncertain,
  Aggregated: QUALITY_TEXT.aggregated,
}

/**
 * 取质量文案：先按原 key 精确匹配（区分大小写的接口），再按小写兜底。
 * 未知值原样返回，不把后端新增状态静默吞掉。
 */
export function qualityText(q: string): string {
  return qualityMap[q] ?? QUALITY_TEXT[q.toLowerCase() as keyof typeof QUALITY_TEXT] ?? q
}

// Element Plus <el-tag> type 接受的字面量联合类型（唯一定义在 statusType.ts）
export type { TagType }

/**
 * 质量 → 徽章色：统一委托全站本源 utils/statusType 的 qualityTagType。
 *
 * 历史页曾有本地覆盖表（aggregated/old=info）与 info→warning 兜底，导致同一质量词
 * 在历史页 / 实时页取色不同（old 尤其明显：QUALITY_TEXT 里 old 与 stale 同为「过期」）。
 * 已收敛为单一口径，这里保留同名导出仅为兼容既有 import，不再持有第二份映射逻辑。
 */
export function qualityTagType(q: string): TagType {
  return statusQualityTagType(q)
}
