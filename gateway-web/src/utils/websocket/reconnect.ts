import { computeReconnectDelay } from './backoff'

/** 重连调度器的宿主回调 */
export interface ReconnectHooks {
  /** 触发一次连接尝试 */
  onConnect: () => void
  /** 重连次数变化通知（用于 UI 展示与 store 上报） */
  onReconnect: (attempts: number) => void
}

/**
 * 重连退避调度：指数退避 + ±30% jitter。
 *
 * S12: 客户端 WebSocket 重连退避（指数 + jitter + 浏览器 online/offline 事件）。
 */
export class ReconnectController {
  private readonly baseDelay = 3000
  private readonly maxDelay = 30000
  private timer: ReturnType<typeof setTimeout> | null = null
  /** 已重连次数（连接成功后归零） */
  attempts = 0
  private readonly hooks: ReconnectHooks

  constructor(hooks: ReconnectHooks) {
    this.hooks = hooks
  }

  /** 清除挂起的重连定时器 */
  clearTimer(): void {
    if (this.timer) {
      clearTimeout(this.timer)
      this.timer = null
    }
  }

  /** 重置重连计数（离线恢复时用，避免 backoff 拉得很长） */
  resetAttempts(): void {
    this.attempts = 0
  }

  /**
   * 排定一次退避重连（已有定时器时不重复排）。
   */
  schedule(): void {
    if (this.timer) return
    this.attempts++
    // f7: 改用 computeReconnectDelay（指数退避 + ±30% jitter）。原内联公式无抖动，
    // 多标签页/多实例在同一时刻断线后会同步重连形成尖峰，打爆服务端。
    const delay = computeReconnectDelay(this.attempts, this.baseDelay, this.maxDelay)
    this.hooks.onReconnect(this.attempts)
    this.timer = setTimeout(() => {
      this.timer = null
      this.hooks.onConnect()
    }, delay)
  }
}
