import type { DataPoint } from '@/api/types'
import { attachSocketHandlers } from './dispatch'
import { buildWsProtocols, buildWsUrl } from './endpoint'
import { HeartbeatController } from './heartbeat'
import { getBrowserNetSnapshot, NetCoordinator } from './net-sensor'
import { ReconnectController } from './reconnect'
import { closeSocketQuietly, detachSocketHandlers } from './socket'
import { publishErrorToStore, publishStatusToStore } from './state'
import { SubscriberRegistry } from './subscribers'
import type { BrowserNetSnapshot, ConnectionState } from './types'

/**
 * 网关 WebSocket 客户端
 * 支持实时数据推送 (/api/realtime) 和日志流 (/api/diagnostics/logs)
 * 认证：通过 URL query 参数 ?token=xxx 传递
 *
 * 桌面端(Tauri)：使用网关实际地址拼接 ws:// URL
 * Web 端：使用 window.location 同源地址
 *
 * 心跳机制：
 *   - 连接 OPEN 后启动定时 ping（默认 30s）
 *   - 服务端应回 pong；若超过 heartbeatTimeout 未收到任何消息，判定连接僵死并主动关闭触发重连
 *   - 心跳生命周期与连接绑定：onclose/close 时立即停止
 *
 * 本类只负责「连接生命周期编排」；URL/子协议、消息解析、心跳、重连退避、
 * 网络感知、订阅表、状态发布等职责见同目录的各子模块。
 */
export class GatewayWebSocket {
  private ws: WebSocket | null = null
  private path: string
  private manualClose = false
  private gen = 0
  // Bug: connectInternal 重入/并发守卫。setupNetworkListeners 的同步首次回调会
  // 触发 handleNetChange → connect() 重入；两次 connectInternal 各自 new WebSocket，
  // 后者 gen 更大，前者创建的连接被 `gen !== this.gen` 守卫全部忽略 → 孤儿 WS。
  private connecting = false
  // B-2：标记上一次 onclose 是否由"网络离线主动关闭"触发。
  // 是 → 不再 scheduleReconnect（避免 30s 退避一直跑）；
  // 否 → 走原有 scheduleReconnect 路径。
  // 每次新 connectInternal() 头部重置 false。
  private fromOffline = false
  private status: ConnectionState = 'disconnected'
  // 上一次已知的浏览器网络状态（null 表示尚未收到过事件）
  private lastNetOnline: boolean | null = null
  // ROI #3 共享连接：多订阅者多播。旧实现只有 onMessage 单槽，多个组件
  // （Realtime.vue / Monitor.vue）各自 new 一个实例才能各收各的消息 →
  // 到同一端点 /api/realtime 建立多条 WS（重复建连）。改为引用计数共享一条
  // 连接后，多个组件通过 addMessageListener/addLogListener 注册回调，互不覆盖。
  private readonly subscribers = new SubscriberRegistry()
  private readonly reconnect: ReconnectController
  private readonly heartbeat: HeartbeatController
  private readonly net: NetCoordinator

  onMessage: ((data: DataPoint[]) => void) | null = null
  onLog: ((line: string) => void) | null = null
  onOpen: (() => void) | null = null
  onClose: (() => void) | null = null
  onError: ((err: Event) => void) | null = null
  onReconnect: ((attempts: number) => void) | null = null
  onHeartbeatTimeout: (() => void) | null = null
  /** 连接状态变化回调（供 UI 连接指示器订阅） */
  onStatusChange: ((s: ConnectionState, attempts?: number) => void) | null = null

  constructor(path: string) {
    this.path = path
    this.heartbeat = new HeartbeatController({
      getSocket: () => this.ws,
      getPath: () => this.path,
      onPing: () => this.sendPing(),
      onTimeout: () => this.onHeartbeatTimeout?.(),
    })
    this.reconnect = new ReconnectController({
      onConnect: () => {
        void this.connect()
      },
      onReconnect: (attempts) => {
        this.onReconnect?.(attempts)
      },
    })
    this.net = new NetCoordinator({
      closeSocket: () => {
        const w = this.ws
        // 先 null 解绑再 close，避免 onclose 处理器走主动关闭分支
        this.ws = null
        if (w) closeSocketQuietly(w)
      },
      resetReconnect: () => {
        this.reconnect.clearTimer()
        this.reconnect.resetAttempts()
      },
      isManualClose: () => this.manualClose,
      markFromOffline: () => {
        this.fromOffline = true
      },
      stopHeartbeat: () => this.stopHeartbeat(),
      setStatus: (s) => this.setStatus(s),
      publishError: (detail) => publishErrorToStore(detail),
      connect: () => {
        void this.connect()
      },
      getLastNetOnline: () => this.lastNetOnline,
      setLastNetOnline: (value) => {
        this.lastNetOnline = value
      },
    })
  }

  /**
   * 注册消息监听回调（共享连接场景用）。返回取消订阅函数（幂等）。
   * 与 `onMessage` 单槽并存，不会互相覆盖。
   */
  addMessageListener(fn: (data: DataPoint[]) => void): () => void {
    return this.subscribers.addMessageListener(fn)
  }

  /** 注册日志流监听回调（共享连接场景用）。返回取消订阅函数（幂等）。 */
  addLogListener(fn: (line: string) => void): () => void {
    return this.subscribers.addLogListener(fn)
  }

  /** 分发实时数据：单槽 + 多播监听，单个订阅者抛错不影响其它订阅者 */
  private emitMessage(data: DataPoint[]): void {
    this.subscribers.emitMessage(data, this.onMessage)
  }

  private async buildUrl(): Promise<string> {
    return buildWsUrl(this.path)
  }

  private async buildProtocols(): Promise<string[]> {
    return buildWsProtocols()
  }

  async connect(): Promise<void> {
    // Bug 226(web): 原 connect() 未捕获自身异常。buildUrl() 在桌面端未配置网关地址时
    // 直接 throw；buildProtocols() 内部 await getToken() 可能 reject；子协议含非法字符时
    // `new WebSocket()` 会同步抛 SyntaxError。而调用方（scheduleReconnect 的定时器回调、
    // online 事件回调、各处 `void ws.connect()`）都不带 catch，异常会变成未处理的
    // Promise rejection：状态永远停在 connecting/reconnecting、重连链断裂、UI 无任何提示。
    // 现统一兜底：上报错误原因 + 状态回落 + 继续按退避重连（manualClose 时不再重连）。
    try {
      await this.connectInternal()
    } catch (e) {
      const detail = e instanceof Error ? e.message : 'WebSocket 连接失败'
      publishErrorToStore(detail)
      if (this.manualClose) {
        this.setStatus('disconnected')
        return
      }
      this.setStatus('reconnecting', this.reconnect.attempts + 1)
      this.reconnect.schedule()
    }
  }

  private async connectInternal(): Promise<void> {
    // Bug: 幂等守卫——同一时刻只允许一次连接流程在跑（详见 `connecting` 字段说明）。
    if (this.connecting) return
    this.connecting = true
    try {
      await this.connectInternalInner()
    } finally {
      this.connecting = false
    }
  }

  private async connectInternalInner(): Promise<void> {
    // S12: 注册浏览器 online/offline 事件——避免断网时无意义重连打爆服务端，
    // 恢复时立即触发一次。
    this.setupNetworkListeners()
    // B-2：每次新连接握手前先读 navigator.onLine；若离线直接断开式返回，
    // 不发起 new WebSocket（TCP SYN 在客户端本地被丢，浪费一次握手）。
    // 离线结束由 BrowserNetSensor online 事件唤醒。
    if (!getBrowserNetSnapshot().online) {
      this.fromOffline = false
      this.setStatus('disconnected')
      publishErrorToStore('网络已断开，恢复后将自动重连')
      // 不 scheduleReconnect——避免在离线状态下一直 backoff 循环。
      // 若恢复由另一实例先连成功，本实例的 store 状态会通过 status
      // 事件被外层订阅者感知（UI 仍会显示 disconnected 直到再触发 connect）。
      return
    }
    // S10: 清除 pending 重连定时器，避免旧 timer 触发再次 connect() 关闭新连接
    this.reconnect.clearTimer()
    if (this.ws) {
      this.manualClose = true
      detachSocketHandlers(this.ws)
      this.ws.close()
      this.ws = null
    }
    this.stopHeartbeat()
    this.manualClose = false
    this.fromOffline = false
    this.setStatus('connecting')
    const gen = ++this.gen
    const url = await this.buildUrl()
    const protocols = await this.buildProtocols()
    // A2 修复：旧实现在 protocols 为空（未登录 / token 缺失 / 字符被过滤光）时
    // 退化为 `new WebSocket(url)` 无子协议握手；若后端允许匿名连接，实时数据将
    // 在无鉴权下暴露。现改为明确拒绝建立连接并暴露状态，登录后由调用方重新触发。
    if (protocols.length === 0) {
      this.setStatus('disconnected')
      publishErrorToStore('未获取到有效凭证，已拒绝建立实时连接，请先登录')
      return
    }
    // B1 修复：通过 Sec-WebSocket-Protocol 子协议传 token，URL 不再含 ?token=。
    const ws = new WebSocket(url, protocols)
    this.ws = ws
    attachSocketHandlers(ws, gen, {
      path: this.path,
      isCurrent: (g, sock) => g === this.gen && this.ws === sock,
      onOpen: () => {
        if (this.reconnect.attempts > 0) {
          this.reconnect.attempts = 0
          this.onReconnect?.(0)
        }
        this.startHeartbeat()
        this.setStatus('connected')
        this.onOpen?.()
      },
      onPong: () => this.heartbeat.notePong(),
      onData: (data) => this.emitMessage(data),
      onLog: (line) => this.subscribers.emitLog(line, this.onLog),
      onClose: () => {
        this.stopHeartbeat()
        this.onClose?.()
        // B-2：offline 主动关闭触发的 ws close 不再 scheduleReconnect
        // （离线状态 backoff 没意义，等 online 事件统一唤醒）。
        if (this.fromOffline && !this.manualClose) {
          this.fromOffline = false
          this.setStatus('disconnected')
          return
        }
        if (!this.manualClose) {
          this.setStatus('reconnecting', this.reconnect.attempts + 1)
          this.reconnect.schedule()
        } else {
          this.setStatus('disconnected')
        }
      },
      onError: (err) => {
        this.onError?.(err)
        publishErrorToStore()
      },
    })
  }

  /** 启动心跳（连接 OPEN 后调用） */
  private startHeartbeat() {
    this.heartbeat.start()
  }

  /** 停止心跳（连接关闭 / 离线时调用） */
  private stopHeartbeat() {
    this.heartbeat.stop()
  }

  /** 发送一次 ping（心跳定时器回调） */
  private sendPing() {
    this.heartbeat.sendPing()
  }

  private setStatus(s: ConnectionState, attempts?: number) {
    if (this.status === s && s !== 'reconnecting') return
    this.status = s
    // 将内部连接态发布到 websocket store（store 仅暴露 3 态）
    publishStatusToStore(s, attempts)
    this.onStatusChange?.(s, attempts)
  }

  // ===== S12 + B-2: 浏览器 online/offline 事件（通过 BrowserNetSensor 统一分发） =====

  private setupNetworkListeners() {
    this.net.setup((snap) => this.handleNetChange(snap))
  }

  /**
   * 处理 BrowserNetSensor 状态变更（B-2 核心逻辑）。
   *
   * 关键设计：
   * - 离线时**主动 close 当前 ws**：避免 onclose 之后再 scheduleReconnect
   *   （90s 心跳兜底时序紧，导致用户在笔记本唤醒后看到 60s 的"假连接"）。
   * - 同一实例多次事件去重（`if (lastNetOnline === snap.online) return`）。
   * - `manualClose=true` 时（用户主动调用 `close()`）一律忽略：避免 UI 关闭后
   *   又被 online 事件唤醒。
   */
  private handleNetChange(snap: BrowserNetSnapshot): void {
    this.net.handleNetChange(snap)
  }

  /** 查询当前连接状态（同步） */
  getStatus(): ConnectionState {
    return this.status
  }

  close() {
    // S12: 清理 online/offline 监听
    this.net.teardown()
    this.manualClose = true
    this.reconnect.resetAttempts()
    this.gen++
    this.setStatus('disconnected')
    this.stopHeartbeat()
    this.reconnect.clearTimer()
    const sock = this.ws
    if (sock) {
      detachSocketHandlers(sock)
      sock.close()
      this.ws = null
    }
  }
}
