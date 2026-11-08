/**
 * 点位表单校验规则与取值收敛
 *
 * 表单校验（必填项）与后端取值约束（采集周期 < 北向上报周期）集中在本模块，
 * 供表单提交、批量导入、行内编辑等处共用。
 */

/**
 * 读不到北向配置时的兜底上报周期（秒）。
 *
 * 沿用历史 60s 语义：点位周期收敛到 59 以内，至少不会把明显非法的值提交上去。
 */
export const FALLBACK_NORTH_INTERVAL = 60

/**
 * 采集周期取值收敛。
 *
 * - 不传 `northInterval`：只保证 >= 1，**不设上界**——编辑既有大周期点位时
 *   不能因为前端猜测的上界把 300s 静默改成 59s（旧实现正是这样改写用户配置）。
 * - 传 `northInterval`：收敛到 [1, northInterval - 1]，因为后端要求
 *   「采集周期 < 北向上报周期」（见 validate_cycle_conflict）。
 */
export function clampInterval(v: number | null | undefined, northInterval?: number): number {
  const n = typeof v === 'number' && Number.isFinite(v) ? Math.trunc(v) : 1
  const lower = Math.max(n, 1)
  if (northInterval === undefined) return lower
  const upper = Math.max(1, Math.trunc(northInterval) - 1)
  return Math.min(lower, upper)
}

/**
 * 单点表单校验规则（Element Plus FormRules）
 */
export function createPointRules() {
  return {
    id: [{ required: true, message: '请输入点位 ID', trigger: 'blur' }],
    bus: [{ required: true, message: '请输入总线', trigger: 'blur' }],
    protocol: [{ required: true, message: '请选择协议', trigger: 'change' }],
    address: [{ required: true, message: '请输入地址', trigger: 'blur' }],
  }
}
