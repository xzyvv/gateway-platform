/**
 * 点位表单校验规则与取值收敛
 *
 * 表单校验（必填项）与后端取值约束（采集周期 1..=59）集中在本模块，
 * 供表单提交、批量导入、行内编辑等处共用。
 */

/** Bug W4: 后端要求点位采集周期 1..=59（0 或 >=60 均返回 4002），取值需收敛。 */
export function clampInterval(v: number | null | undefined): number {
  const n = typeof v === 'number' && Number.isFinite(v) ? Math.trunc(v) : 1
  return Math.min(Math.max(n, 1), 59)
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
