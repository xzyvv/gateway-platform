import type { DataPoint } from '@/api/types'
import { logError } from '../logger'
import type { DataPointListener, LogListener } from './types'

/**
 * 订阅者注册表：共享连接场景下的多播订阅。
 *
 * 约束：
 * - 单槽回调（`onMessage` / `onLog`）与多播监听并存，互不覆盖。
 * - 单个订阅者抛错被隔离，不影响其它订阅者（与帧观察者一致）。
 */
export class SubscriberRegistry {
  private messageListeners = new Set<DataPointListener>()
  private logListeners = new Set<LogListener>()

  /** 注册消息监听回调，返回取消订阅函数（幂等） */
  addMessageListener(fn: DataPointListener): () => void {
    this.messageListeners.add(fn)
    return () => {
      this.messageListeners.delete(fn)
    }
  }

  /** 注册日志流监听回调，返回取消订阅函数（幂等） */
  addLogListener(fn: LogListener): () => void {
    this.logListeners.add(fn)
    return () => {
      this.logListeners.delete(fn)
    }
  }

  /** 分发实时数据：单槽 + 多播监听，单个订阅者抛错不影响其它订阅者 */
  emitMessage(data: DataPoint[], slot?: DataPointListener | null): void {
    slot?.(data)
    for (const fn of Array.from(this.messageListeners)) {
      try {
        fn(data)
      } catch (e) {
        logError('ws', e, '[ws] message listener threw')
      }
    }
  }

  /** 分发日志行：单槽 + 多播监听，单个订阅者抛错不影响其它订阅者 */
  emitLog(line: string, slot?: LogListener | null): void {
    slot?.(line)
    for (const fn of Array.from(this.logListeners)) {
      try {
        fn(line)
      } catch (e) {
        logError('ws', e, '[ws] log listener threw')
      }
    }
  }
}
