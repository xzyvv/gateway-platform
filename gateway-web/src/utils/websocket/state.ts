import { useWebsocketStore } from '../../stores/websocket'
import type { ConnectionState } from './types'

/**
 * 连接状态管理：把内部 4 态连接态发布到 websocket store（store 仅暴露 3 态）。
 */

/** 将内部连接态发布到 websocket store（store 仅暴露 3 态） */
export function publishStatusToStore(s: ConnectionState, attempts?: number): void {
  const store = useWebsocketStore()
  switch (s) {
    case 'connected':
      store.setStatus('connected')
      store.resetReconnect()
      store.setLastError(null)
      // 仅当之前有重连历史时（attempts 或历史非空）才记录恢复事件，
      // 否则每次启动成功都刷一条 history 噪音太大。
      if (attempts && attempts > 0) {
        store.addReconnectEvent({ kind: 'recovered' })
      }
      break
    case 'connecting':
      store.setStatus('connecting')
      break
    case 'reconnecting':
      store.setStatus('connecting')
      if (typeof attempts === 'number' && attempts > 0) {
        store.incrementReconnect()
        store.addReconnectEvent({ kind: 'reconnecting', attempt: attempts })
      }
      break
    case 'disconnected':
      store.setStatus('disconnected')
      store.resetReconnect()
      break
  }
}

/** 上报错误原因到 websocket store */
export function publishErrorToStore(detail?: string): void {
  const msg = detail ?? 'WebSocket 连接发生错误'
  const store = useWebsocketStore()
  store.setLastError(msg)
  store.addReconnectEvent({ kind: 'error', message: msg })
}
