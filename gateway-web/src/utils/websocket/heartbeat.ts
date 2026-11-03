import { emitWsFrame } from './frames'

/** 心跳控制器的宿主回调 */
export interface HeartbeatHooks {
  /** 取当前底层 socket（未连接时为 null） */
  getSocket: () => WebSocket | null
  /** 当前连接路径（用于帧观察） */
  getPath: () => string
  /** 发送一次 ping（由心跳定时器调用） */
  onPing: () => void
  /** 判定连接僵死时的回调（通常触发 UI 提示） */
  onTimeout: () => void
}

/**
 * 心跳 ping/pong 与僵死判定。
 *
 * 心跳是「连接自身」的基础设施，生命周期跟随 connect/close 而非组件挂载，
 * 因此不走 usePolling 的组件调度器（W2-#12 收口时的有意例外）。
 */
export class HeartbeatController {
  private readonly interval = 30000
  // Bug P1-B8 (fix): heartbeatTimeout 60s → 90s，并叠加「连续 N 次未收到 pong」
  // 判定（默认 2 次）。单次窗口延长给服务端繁忙 + 网络抖动留余量；连续 N
  // 次保护防止偶发单次延迟直接误断。
  private readonly timeout = 90000
  private readonly missLimit = 2
  private timer: ReturnType<typeof setInterval> | null = null
  private timeoutTimer: ReturnType<typeof setTimeout> | null = null
  // Bug P1-B8 (fix): 心跳存活只认「最近一次收到 pong 的时刻」，不再用
  // 「任意消息到达时刻」——服务端 data 推送会掩盖真断连。
  private lastPongAt = 0
  private consecutivePongMisses = 0
  private readonly hooks: HeartbeatHooks

  constructor(hooks: HeartbeatHooks) {
    this.hooks = hooks
  }

  /** 启动心跳（连接 OPEN 后调用） */
  start(): void {
    this.stop()
    // Bug P1-B8 (fix): 启动时把 lastPongAt 设为 now（视为刚收到 pong），
    // 避免刚连上就触发「连续 N 次未收到 pong」。
    this.lastPongAt = Date.now()
    this.consecutivePongMisses = 0
    this.timer = setInterval(() => {
      this.hooks.onPing()
    }, this.interval)
    this.scheduleTimeoutCheck()
  }

  /** 停止心跳（连接关闭时调用） */
  stop(): void {
    if (this.timer) {
      clearInterval(this.timer)
      this.timer = null
    }
    if (this.timeoutTimer) {
      clearTimeout(this.timeoutTimer)
      this.timeoutTimer = null
    }
  }

  /** 收到 pong：刷新存活时间并清零连续 miss 计数 */
  notePong(): void {
    this.lastPongAt = Date.now()
    this.consecutivePongMisses = 0
  }

  /** 发送一次 ping（连接未就绪时跳过） */
  sendPing(): void {
    const sock = this.hooks.getSocket()
    if (!sock || sock.readyState !== WebSocket.OPEN) return
    const payload = JSON.stringify({ type: 'ping', ts: Date.now() })
    try {
      sock.send(payload)
      emitWsFrame(this.hooks.getPath(), 'out', 'ping', payload)
    } catch {
      /* 发送失败忽略，等待下次心跳 */
    }
  }

  private scheduleTimeoutCheck(): void {
    if (this.timeoutTimer) clearTimeout(this.timeoutTimer)
    this.timeoutTimer = setTimeout(() => {
      // Bug P1-B8 (fix): 判定改为「自上次 pong 起超时 + 连续 N 次」。
      // 只看 lastActivityAt 会被服务端 data 推送掩盖，单独追踪 lastPongAt。
      const pongIdle = Date.now() - this.lastPongAt
      if (pongIdle >= this.timeout) {
        this.consecutivePongMisses++
        if (this.consecutivePongMisses >= this.missLimit) {
          this.hooks.onTimeout()
          const sock = this.hooks.getSocket()
          if (sock && sock.readyState === WebSocket.OPEN) {
            sock.close()
          }
          return
        }
      } else {
        // 期间收到过 pong，misses 已在 notePong 处清零
      }
      this.scheduleTimeoutCheck()
    }, this.timeout)
  }
}
