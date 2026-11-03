import type { DataPoint } from '@/api/types'
import { emitWsFrame } from './frames'

/**
 * 消息解析与分发：把服务端下发的原始帧解析成结构化结果并投递给订阅者。
 */

/** 解析后的入站消息分类 */
export type ParsedMessage =
  /** 心跳 pong（属于心跳噪声，不计入帧观察） */
  | { kind: 'pong' }
  /** 实时数据点数组 */
  | { kind: 'data'; data: DataPoint[]; text: string }
  /** 业务对象（日志 / 事件） */
  | { kind: 'object'; frameType: string; text: string }
  /** 非 JSON 文本 */
  | { kind: 'text'; text: string }

/**
 * 解析服务端下发的原始帧：
 * - JSON 解析失败 → 文本帧（当作日志行处理）。
 * - `type === 'pong'` → 心跳应答（单独刷新心跳存活时间，不分发）。
 * - JSON 数组 → 实时数据点。
 * - JSON 对象 → 业务日志 / 事件。
 * - JSON 标量 → 文本帧。
 */
export function parseIncomingMessage(raw: unknown): ParsedMessage {
  let data: unknown
  try {
    data = JSON.parse(raw as string)
  } catch {
    return { kind: 'text', text: String(raw) }
  }
  if (data && typeof data === 'object' && (data as { type?: string }).type === 'pong') {
    return { kind: 'pong' }
  }
  const text = typeof raw === 'string' ? raw : JSON.stringify(data)
  if (Array.isArray(data)) {
    return { kind: 'data', data: data as DataPoint[], text }
  }
  if (data && typeof data === 'object') {
    return { kind: 'object', frameType: (data as { type?: string }).type ?? 'log', text }
  }
  return { kind: 'text', text: String(raw) }
}

/** socket 事件处理器的宿主回调 */
export interface SocketHandlerHooks {
  /** 当前连接路径（用于帧观察） */
  path: string
  /** 世代守卫：只有仍是当前连接才处理事件 */
  isCurrent: (gen: number, sock: WebSocket) => boolean
  /** 连接建立 */
  onOpen: () => void
  /** 收到 pong（心跳应答） */
  onPong: () => void
  /** 收到实时数据帧 */
  onData: (data: DataPoint[]) => void
  /** 收到日志 / 业务帧 */
  onLog: (line: string) => void
  /** 连接关闭 */
  onClose: () => void
  /** 连接错误 */
  onError: (err: Event) => void
}

/**
 * 给底层 socket 挂载事件处理器。
 *
 * 世代守卫（`isCurrent`）在此统一收口：旧连接的迟到事件被直接忽略，
 * 避免孤儿连接污染当前连接的状态与消息分发。
 */
export function attachSocketHandlers(sock: WebSocket, gen: number, hooks: SocketHandlerHooks): void {
  sock.onopen = () => {
    if (!hooks.isCurrent(gen, sock)) return
    hooks.onOpen()
  }

  sock.onmessage = (event) => {
    if (!hooks.isCurrent(gen, sock)) return
    const parsed = parseIncomingMessage(event.data)
    switch (parsed.kind) {
      case 'pong':
        // Bug P1-B8 (fix): pong 到达时单独刷新 lastPongAt + 清零连续 miss。
        hooks.onPong()
        return
      case 'data':
        emitWsFrame(hooks.path, 'in', 'data', parsed.text)
        hooks.onData(parsed.data)
        return
      case 'object':
        emitWsFrame(hooks.path, 'in', parsed.frameType, parsed.text)
        hooks.onLog(parsed.text)
        return
      case 'text':
        emitWsFrame(hooks.path, 'in', 'text', parsed.text)
        hooks.onLog(parsed.text)
        return
    }
  }

  sock.onclose = () => {
    if (!hooks.isCurrent(gen, sock)) return
    hooks.onClose()
  }

  sock.onerror = (err) => {
    if (!hooks.isCurrent(gen, sock)) return
    hooks.onError(err)
  }
}
