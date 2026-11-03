import type { DataPoint } from '@/api/types'

/**
 * 网关 WebSocket 客户端的类型定义。
 *
 * 这里只放「跨模块共享」的类型：连接状态、浏览器网络快照、WS 帧信息。
 * 其余类型跟随各自的实现模块（心跳 / 分发 / 订阅表 / 连接池）。
 */

/** WebSocket 连接状态（供 UI 显示连接指示器） */
export type ConnectionState = 'connecting' | 'connected' | 'disconnected' | 'reconnecting'

/** 网关 WebSocket 离线原因标签（仅给上层观察用） */
export interface BrowserNetSnapshot {
  /** 当前 navigator.onLine（best-effort 浏览器判定） */
  online: boolean
  /** 自上次 offline 起的毫秒数；在线时为 0 */
  offlineMs: number
}

/**
 * WebSocket 帧观察者——只给请求监视器采样用，不参与业务消息分发。
 *
 * 约束：
 * - 无订阅者时零开销：emit 直接返回，不构造预览字符串。
 * - 观察者抛错被隔离，不影响 messageListeners / 日志管线。
 * - 只上报可读帧：入站数据/日志帧与出站 ping；pong 属于心跳噪声，不记录。
 */
export interface WsFrameInfo {
  /** WS 端点路径，如 /api/realtime */
  path: string
  direction: 'in' | 'out'
  /** 帧类型：data / ping / 业务 type / text */
  type: string
  /** 原始帧字符数 */
  size: number
  /** 原始帧内容（超长截断） */
  data: string
}

/** 实时数据点回调签名 */
export type DataPointListener = (data: DataPoint[]) => void

/** 日志行回调签名 */
export type LogListener = (line: string) => void
