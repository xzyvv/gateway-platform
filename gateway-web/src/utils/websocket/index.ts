/**
 * 网关 WebSocket 客户端（统一出口）。
 *
 * 拆分前 `src/utils/websocket.ts` 对外暴露的符号在此原样再导出：
 * 类型、函数、类、测试辅助入口的行为与签名均未改变，
 * 因此全站 `from '@/utils/websocket'` 的调用方无需修改。
 */

// ===== 类型 =====
export type { ConnectionState, BrowserNetSnapshot, WsFrameInfo } from './types'
export type { SharedWebSocketHandle } from './pool'

// ===== 重连退避 =====
export { computeReconnectDelay } from './backoff'

// ===== WS 帧观察者 =====
export { subscribeWsFrames } from './frames'

// ===== 浏览器网络状态传感器 =====
export {
  getBrowserNetSnapshot,
  subscribeBrowserNet,
  __resetBrowserNetSensorForTests,
  __setNavigatorOnlineForTests,
} from './net-sensor'

// ===== 连接客户端 =====
export { GatewayWebSocket } from './connection'

// ===== 共享连接池 =====
export {
  acquireSharedWebSocket,
  forceReconnectAll,
  __resetSharedWebSocketPoolForTests,
  __sharedWebSocketCountForTests,
} from './pool'
