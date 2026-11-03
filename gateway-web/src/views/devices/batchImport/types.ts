/**
 * 批量导入页的共享类型。
 *
 * 设备分支与点位分支各自维护状态，但行结构、导入结果、导入历史这几类
 * 数据在两个分支间语义一致，集中在本文件避免各写一份形状相同的 type。
 */
import type { PointConfig } from '@/api/types'

/** 逐行导入结果（成功 / 失败原因） */
export type OutcomeRow = { line: number; name: string; id: string; ok: boolean; error?: string }

/** 本次会话的导入历史行（结果来自真实接口，不做持久化） */
export type HistoryRow = { time: number; fileName: string; total: number; success: number; failed: number }

/** 点位校验预览行（CSV / JSON 两条路径共用） */
export interface PointRow extends Partial<PointConfig> {
  line: number
  valid: boolean
  error?: string
}

/** 点位校验结果汇总 */
export interface PointParseResult {
  total: number
  valid: number
  invalid: number
  rows: PointRow[]
}
