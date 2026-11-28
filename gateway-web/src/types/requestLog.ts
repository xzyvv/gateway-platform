/**
 * 请求监视器的单条记录类型。
 *
 * 定义下沉到 types 层（DD-86）：stores 与 utils/har 都要用，放在 store 里会形成
 * `utils -> stores` 的层级倒挂；store 负责状态，类型本身是纯数据契约。
 */
export interface RequestLogEntry {
  id: number
  method: string
  url: string
  status: number | string
  /** 耗时（毫秒） */
  duration: number
  /** 请求发起时间戳（ms），与 startTime 一致 */
  timestamp: number
  startTime: number
  endTime: number
  requestData?: unknown
  responseData?: unknown
  error?: string
  /** 后端 trace ID（响应头 x-request-id），报障时贴给研发即可定位到日志 */
  traceId?: string
  /** 实际发出的请求头（敏感值已脱敏） */
  requestHeaders?: Array<{ name: string; value: string }>
  /** 实际收到的响应头（敏感值已脱敏） */
  responseHeaders?: Array<{ name: string; value: string }>
  /** 记录来源：http（默认）或 ws 帧 */
  kind?: 'http' | 'ws'
  /** WS 帧明细（kind === 'ws' 时存在） */
  ws?: {
    direction: 'in' | 'out'
    type: string
    size: number
    data: string
  }
}
