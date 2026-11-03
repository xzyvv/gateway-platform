import { logError } from '../logger'
import type { WsFrameInfo } from './types'

/**
 * WebSocket 帧观察者实现（类型见 `types.ts` 的 `WsFrameInfo`）。
 *
 * 只给请求监视器采样用，不参与业务消息分发。
 */

type WsFrameListener = (frame: WsFrameInfo) => void

const wsFrameListeners = new Set<WsFrameListener>()
const WS_FRAME_PREVIEW_CHARS = 4096

/** 订阅 WS 帧，返回取消订阅函数 */
export function subscribeWsFrames(fn: WsFrameListener): () => void {
  wsFrameListeners.add(fn)
  return () => {
    wsFrameListeners.delete(fn)
  }
}

/** 向帧观察者广播一帧（内部使用；pong 属于心跳噪声，不上报） */
export function emitWsFrame(path: string, direction: 'in' | 'out', type: string, raw: string): void {
  if (wsFrameListeners.size === 0) return
  const data = raw.length > WS_FRAME_PREVIEW_CHARS
    ? `${raw.slice(0, WS_FRAME_PREVIEW_CHARS)}…[truncated ${raw.length} chars]`
    : raw
  const frame: WsFrameInfo = { path, direction, type, size: raw.length, data }
  for (const fn of Array.from(wsFrameListeners)) {
    try {
      fn(frame)
    } catch (e) {
      logError('ws', e, '[ws] frame listener threw')
    }
  }
}
