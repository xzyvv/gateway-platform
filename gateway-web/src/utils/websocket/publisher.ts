/**
 * WebSocket 连接态的发布端口（DD-86）。
 *
 * `connection.ts` / `pool.ts` 位于 utils 层，需要把连接状态、活跃流信息同步到
 * websocket store；直接 import store 会形成 `utils -> stores` 倒挂。
 * 改为由 stores/websocket.ts 在加载时注册实现，utils 只调用这里的接口；
 * 未注册时（纯单测、无 Pinia）静默丢弃，不抛错。
 */
import type { ConnectionState } from './types'

export interface WsStorePublisher {
  /** 发布连接态（store 仅暴露 3 态，映射逻辑在 stores 层） */
  status: (s: ConnectionState, attempts?: number) => void
  /** 发布错误原因 */
  error: (detail?: string) => void
  /** 登记/注销活跃实时流（供顶栏 WS 徽章区分「无实时功能」与「假断开」） */
  addStream: (path: string) => void
  removeStream: (path: string) => void
}

let publisher: WsStorePublisher | null = null

export function setWsStorePublisher(p: WsStorePublisher): void {
  publisher = p
}

/** 将内部连接态发布到 websocket store（未注册时为 no-op） */
export function publishStatusToStore(s: ConnectionState, attempts?: number): void {
  publisher?.status(s, attempts)
}

/** 上报错误原因到 websocket store（未注册时为 no-op） */
export function publishErrorToStore(detail?: string): void {
  publisher?.error(detail)
}

/** 登记活跃实时流 */
export function addActiveStream(path: string): void {
  publisher?.addStream(path)
}

/** 注销活跃实时流 */
export function removeActiveStream(path: string): void {
  publisher?.removeStream(path)
}
