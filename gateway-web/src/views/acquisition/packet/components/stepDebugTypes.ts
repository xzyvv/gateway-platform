/**
 * 单步调试（StepDebug）的共享类型与错误码文案
 *
 * 页面主组件与两个面板（components/StepSequencePanel.vue 执行区、
 * components/StepRunLogPanel.vue 日志区）都要用到「单步执行结果」结构与
 * 「后端错误码 → 中文标签」映射，抽到独立模块避免面板之间互相 import。
 */

/** 单步执行结果状态 */
export type OutcomeStatus = 'ok' | 'fail' | 'skipped'

/** 单步执行结果：驱动左侧步骤状态标签与进度 */
export interface StepOutcome {
  status: OutcomeStatus
  requestHex: string
  responseHex: string
  parsed: string
  latencyMs: number
  error: string | null
  at: number
}

/** 执行日志条目：在单步结果基础上补步骤序号与所属总线 */
export interface RunLog extends StepOutcome {
  key: string
  step: number
  bus: string
}

/**
 * 「后端错误码 → 中文标签」的唯一实现在 `utils/protocolFailures.ts`。
 *
 * 这里曾自带一份 ERROR_LABEL，与 `packet/PacketEditor.vue` 里的另一份对同一个
 * 机器码给出不同说法（bus_offline 一处「总线离线」一处「总线不可用」…），
 * 用户会以为是两种故障。现统一复用共享映射，本模块只保留类型定义并转出，
 * 避免调用方为了一个文案函数多 import 一个路径。
 */
export { protocolFailureLabel as errorLabel } from '@/utils/health/protocolFailures'
