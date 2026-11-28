/**
 * HTTP 层的 UI 事件端口（③ api → stores 事件化）。
 *
 * axios 拦截器要「写通知中心 + 写请求日志」，但 api 是最底层，不能静态依赖 Pinia store。
 * 改为：api 只发事件，stores/httpEvents.ts 在加载时注册处理器并落库。
 * 未注册时（纯 api 单测、无 Pinia）静默丢弃，不抛错。
 */
import type { RequestLogEntry } from '@/types/requestLog'

export interface HttpNotifyOpts {
  code?: number
  silent?: boolean
  traceId?: string
}

export interface HttpNotifyEvent {
  level: 'success' | 'error' | 'warning' | 'info'
  message: string
  opts?: HttpNotifyOpts
}

/** 与 requestLog store 的 pushLog 入参一致（id 由 store 生成，timestamp 可选） */
export type RequestLogPayload = Omit<RequestLogEntry, 'id' | 'timestamp'> & { timestamp?: number }

export interface HttpEventsHandlers {
  notify?: (e: HttpNotifyEvent) => void
  logRequest?: (entry: RequestLogPayload) => void
}

let handlers: HttpEventsHandlers = {}

export function setHttpEventsHandlers(h: HttpEventsHandlers): void {
  handlers = h
}

/** 发一条全局通知（由通知中心 store 决定落库与是否弹窗） */
export function emitHttpNotify(e: HttpNotifyEvent): void {
  handlers.notify?.(e)
}

/** 记一条请求日志（由 requestLog store 落库，尊重其关闭开关） */
export function emitRequestLog(entry: RequestLogPayload): void {
  handlers.logRequest?.(entry)
}
