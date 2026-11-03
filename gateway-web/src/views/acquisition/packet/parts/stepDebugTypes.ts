/**
 * 单步调试（StepDebug）的共享类型与错误码文案
 *
 * 页面主组件与两个面板（parts/StepSequencePanel.vue 执行区、
 * parts/StepRunLogPanel.vue 日志区）都要用到「单步执行结果」结构与
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

/** 后端错误码 → 中文标签 */
export const ERROR_LABEL: Record<string, string> = {
  timeout: '超时无响应',
  crc: 'CRC 校验失败',
  exception: '设备返回异常码',
  bus_offline: '总线离线',
  bus_busy: '总线被占用',
  invalid_frame: '帧格式非法',
  bus_not_found: '总线不存在',
}

/** 错误码转中文标签；未知错误码原样返回 */
export function errorLabel(err?: string | null): string {
  if (!err) return ''
  return ERROR_LABEL[err] ?? err
}
