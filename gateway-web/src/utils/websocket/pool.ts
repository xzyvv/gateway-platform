import { useWebsocketStore } from '../../stores/websocket'
import { GatewayWebSocket } from './connection'

// ===== ROI #3: 共享连接池（按 path 复用同一 GatewayWebSocket，避免重复建连） =====
//
// 旧实现：Realtime.vue（经 useRealtime）与 Monitor.vue 都订阅 `/api/realtime`，
// 但各自 `new GatewayWebSocket('/api/realtime')` → 到同一端点建立两条独立 WS：
//   - 网关侧看到重复订阅者，本机多占一条连接与心跳；
//   - 两个实例各自一套 online/offline 监听与重连退避，切换网络时同步抖动。
// 现改为引用计数共享：同一 path 首个使用者建立连接，其余复用；引用归零后关闭。

export interface SharedWebSocketHandle {
  /** 共享的底层连接实例 */
  ws: GatewayWebSocket
  /** 释放一次引用；引用归零时关闭底层连接并从池中移除（幂等） */
  release: () => void
}

interface SharedEntry {
  ws: GatewayWebSocket
  refs: number
}

const sharedPool = new Map<string, SharedEntry>()

/**
 * 按 path 获取共享 WebSocket 连接（引用计数）。
 *
 * - 池中不存在该 path → 新建实例并立即 `connect()`；存在 → 直接复用，不重复握手。
 * - 每次调用 `refs++`；调用返回的 `release()` 使 `refs--`，归零时关闭连接并移除条目，
 *   下一次 acquire 会重新建立连接。
 * - 生命周期：调用方在 `onMounted` acquire、`onUnmounted` release。
 */
export function acquireSharedWebSocket(path: string): SharedWebSocketHandle {
  let entry = sharedPool.get(path)
  if (!entry) {
    entry = { ws: new GatewayWebSocket(path), refs: 0 }
    sharedPool.set(path, entry)
    // 注册到 store：告知「有实时流实例在跑」，供顶栏 WS 徽章区分
    // 「断开需提示」与「页面无实时功能不该提示假断开」
    store().addActiveStream(path)
    // 首个使用者触发建连；后续复用的使用者不再触发 connect，避免打断已建立的连接
    void entry.ws.connect()
  }
  entry.refs++
  const acquired = entry
  // 每个句柄只允许释放一次：重复释放（如重复 unmount / 手滑调用两次）不应
  // 把引用多减一次、进而误关其它订阅者仍在使用的连接。
  let released = false
  return {
    ws: acquired.ws,
    release: () => {
      if (released) return
      released = true
      const cur = sharedPool.get(path)
      // 已被后续 acquire 替换（上一轮已归零关闭）时直接忽略
      if (!cur || cur !== acquired) return
      cur.refs--
      if (cur.refs <= 0) {
        sharedPool.delete(path)
        store().removeActiveStream(path)
        cur.ws.close()
      }
    },
  }
}

/** 延迟取 store：避免模块加载期 pinia 尚未安装 */
function store() {
  return useWebsocketStore()
}

/** 测试用：清空共享连接池并关闭所有连接 */
export function __resetSharedWebSocketPoolForTests(): void {
  for (const e of sharedPool.values()) e.ws.close()
  sharedPool.clear()
  try {
    const s = store()
    for (const p of Array.from(s.activeStreams)) s.removeActiveStream(p)
  } catch {
    /* 测试环境可能未安装 pinia，忽略 */
  }
}

/**
 * 手动触发所有共享 WS 连接的强制重连。
 * Topbar WS 状态徽章的「立即重连」按钮调用此函数。
 * GatewayWebSocket.connect() 内部有幂等守卫（connecting 标志），
 * 对已在连接中的实例直接跳过，不会产生重复握手。
 */
export function forceReconnectAll(): number {
  let n = 0
  for (const e of sharedPool.values()) {
    void e.ws.connect()
    n++
  }
  return n
}

/** 测试用：当前池中活跃 path 数量 */
export function __sharedWebSocketCountForTests(): number {
  return sharedPool.size
}
