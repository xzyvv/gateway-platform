/**
 * 虚拟点位求值状态分类。
 *
 * 后端把「尚未求值」也如实放在 `error` 字段里上报（不静默伪装成功），
 * 前端必须区分「数据还没来」与「公式真的坏了」，否则刚建的点位会被标成
 * 红色「求值异常」，误导运维去改一个根本没问题的公式。
 *
 * 分类口径与后端一致：
 * - 后端 `尚未求值（等待采集数据）` → `pending`（等首批采集数据）
 * - 后端 `引用点位不可用: xxx` / `引用点位不可用（未采集 / 已过期 / 存在循环引用）` → `waiting`
 * - 后端 `公式语法错误: ...` / `公式求值失败: ...` → `error`
 * - 虚拟点位 `enabled=false` → `disabled`
 */

export type VirtualPointEvalStatus = 'ok' | 'pending' | 'waiting' | 'error' | 'disabled'

export interface VirtualPointEvalInput {
  enabled: boolean
  lastValue: number | null
  error: string | null
}

export interface VirtualPointEvalUi {
  text: string
  type: 'success' | 'info' | 'warning' | 'danger'
}

const UI: Record<VirtualPointEvalStatus, VirtualPointEvalUi> = {
  ok: { text: '求值中', type: 'success' },
  pending: { text: '待采集', type: 'info' },
  waiting: { text: '等待引用值', type: 'warning' },
  error: { text: '求值异常', type: 'danger' },
  disabled: { text: '已停用', type: 'info' },
}

/** 把虚拟点位的后端状态归类为展示状态 */
export function virtualPointEvalStatus(p: VirtualPointEvalInput): VirtualPointEvalStatus {
  if (!p.enabled) return 'disabled'
  if (p.lastValue !== null) return 'ok'
  const err = p.error
  if (!err) return 'pending'
  if (err === 'disabled') return 'disabled'
  if (err.startsWith('尚未求值')) return 'pending'
  if (err.startsWith('引用点位不可用')) return 'waiting'
  return 'error'
}

/** 取展示文案与配色 */
export function virtualPointEvalUi(p: VirtualPointEvalInput): VirtualPointEvalUi {
  return UI[virtualPointEvalStatus(p)]
}
