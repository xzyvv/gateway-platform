import { defineStore } from 'pinia'
import { ref, computed } from 'vue'

export type WebsocketStatus = 'connected' | 'connecting' | 'disconnected'

/** 重连历史事件类型：用于 Topbar WS 状态徽章下拉展示 */
export type ReconnectEventKind = 'error' | 'reconnecting' | 'recovered'

export interface ReconnectEvent {
  /** 事件发生的时间戳（ms） */
  time: number
  /** 事件类型：error=连接错误 / reconnecting=第 N 次重连调度 / recovered=重连成功 */
  kind: ReconnectEventKind
  /** 重连序号（仅 reconnecting 时有） */
  attempt?: number
  /** 错误信息（仅 error 时有） */
  message?: string
}

const MAX_HISTORY = 10

/**
 * WebSocket 连接状态 Store
 *
 * 集中发布网关 WebSocket 的连接状态，供 UI 连接指示器响应式订阅。
 * - status：连接态（connected / connecting / disconnected）
 * - reconnectCount：当前断连会话已尝试的重连次数（连接成功或手动关闭后归零）
 * - lastError：最近一次连接错误信息（连接成功后清空）
 * - reconnectHistory：重连/错误/恢复事件环形队列（最近 10 条）
 * - subscribers：当前活跃的 WebSocket 连接订阅数（页面挂载 subscribe，卸载 unsubscribe）
 *
 * GatewayWebSocket 在状态变更时调用 actions 发布到本 store，
 * 页面/组件通过 storeToRefs 读取响应式状态，替代原先分散的 onStatusChange 回调。
 *
 * 注意：内部 ConnectionState 含 4 态（多一个 reconnecting），
 * 发布时 reconnecting 被映射为 connecting + reconnectCount>0（见 websocket.ts publishStatus）。
 */
export const useWebsocketStore = defineStore('websocket', () => {
  const status = ref<WebsocketStatus>('disconnected')
  const reconnectCount = ref(0)
  const lastError = ref<string | null>(null)
  const subscribers = ref(0)
  const reconnectHistory = ref<ReconnectEvent[]>([])
  /** 当前有实例在跑的实时流端点集合（如 /api/realtime）。
   *  顶栏 WS 徽章据此区分「有实时流在跑但断开了」（该提示）与
   *  「当前页面根本没有实时功能」（不该提示，否则任何页面都常驻假「已断开」）。 */
  const activeStreams = ref(new Set<string>())

  const isConnected = computed(() => status.value === 'connected')
  /** connecting 且已有重连记录视为重连中 */
  const isReconnecting = computed(() => status.value === 'connecting' && reconnectCount.value > 0)

  function addActiveStream(path: string) {
    activeStreams.value.add(path)
  }

  function removeActiveStream(path: string) {
    activeStreams.value.delete(path)
  }

  function setStatus(s: WebsocketStatus) {
    status.value = s
  }

  function incrementReconnect() {
    reconnectCount.value++
  }

  function resetReconnect() {
    reconnectCount.value = 0
  }

  function setLastError(msg: string | null) {
    lastError.value = msg
  }

  /** 往重连历史队列加一条事件，超出上限自动丢弃最早的 */
  function addReconnectEvent(evt: Omit<ReconnectEvent, 'time'>) {
    reconnectHistory.value.unshift({ ...evt, time: Date.now() })
    if (reconnectHistory.value.length > MAX_HISTORY) {
      reconnectHistory.value.length = MAX_HISTORY
    }
  }

  function clearHistory() {
    reconnectHistory.value = []
  }

  function subscribe() {
    subscribers.value++
  }

  function unsubscribe() {
    if (subscribers.value > 0) subscribers.value--
  }

  return {
    status,
    reconnectCount,
    lastError,
    subscribers,
    reconnectHistory,
    activeStreams,
    isConnected,
    isReconnecting,
    addActiveStream,
    removeActiveStream,
    setStatus,
    incrementReconnect,
    resetReconnect,
    setLastError,
    addReconnectEvent,
    clearHistory,
    subscribe,
    unsubscribe,
  }
})