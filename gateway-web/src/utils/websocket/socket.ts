/**
 * 底层 WebSocket socket 的通用工具。
 */

/** 解绑 socket 事件处理器（避免 close 之后仍触发回调） */
export function detachSocketHandlers(sock: WebSocket): void {
  sock.onopen = null
  sock.onmessage = null
  sock.onclose = null
  sock.onerror = null
}

/**
 * 关闭底层 socket 并吞掉异常。
 *
 * 离线主动关闭时使用：ws 可能已处于 CLOSING/CLOSED，close() 抛错不应影响状态收敛。
 */
export function closeSocketQuietly(sock: WebSocket): void {
  try {
    detachSocketHandlers(sock)
    sock.close()
  } catch {
    /* ignore: ws 可能已经处于 CLOSING/CLOSED */
  }
}
