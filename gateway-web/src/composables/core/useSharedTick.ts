import { onMounted, onUnmounted } from 'vue'
import { usePolling, __resetUsePollingForTests } from '@/composables/core/usePolling'
import { logError } from '@/utils/logger'

/**
 * 全站共享 tick（1Hz 级展示刷新）
 *
 * 为什么需要：
 * 顶栏时钟、系统总览时钟、Dashboard 的「x 秒前」文案、表格里的 FreshnessBadge
 * 此前各自持有 setInterval(1000)。FreshnessBadge 是**按行渲染**的组件，50 行数据
 * 就是 50 个定时器、每秒 50 次组件重渲染；再加 3 个单例定时器，对一台 7×24 常开的
 * 网关控制台来说全是无谓唤醒。
 *
 * 与 usePolling 的关系（W2-#12 收口）：
 * 两者不再各持一套调度器。usePolling 是唯一的共享定时器实现（共享单例、隐藏暂停、
 * 可见性补跑、并发闸门），本模块只是把它包装成「展示刷新」的语义：
 *   - 不发请求、失败不退避（backoff=false）：展示回调没有「网关失联」的概念；
 *   - 回调异常走统一日志（onError），不连累其他订阅者；
 *   - 手动 start/stop 与 onMounted/onUnmounted 对齐，首轮等待满一个周期
 *     （调用方通常自己先做一次同步初始化）。
 *
 * 为什么用模块级状态：这里存的是调度器本身（定时器句柄 + 订阅集合），不是响应式
 * 业务状态，不存在跨页面数据一致性问题；改成 Pinia store 只会把定时器塞进 devtools
 * 且多一层生命周期。订阅关系的创建/销毁仍由各组件自己的 onMounted/onUnmounted 驱动。
 */

export interface UseSharedTickReturn {
  /** 手动启动（幂等）；正常场景由 onMounted 自动调用 */
  start: () => void
  /** 手动停止并摘除订阅；正常场景由 onUnmounted 自动调用 */
  stop: () => void
  /** 调整本订阅者的刷新周期（毫秒，下限 100）；用于按数据新鲜度降频 */
  setInterval: (ms: number) => void
  /** 本订阅者当前是否在册（测试钩子） */
  active: () => boolean
}

/**
 * 订阅共享 tick。
 *
 * @param run 每次到点执行的回调（同步、轻量；重活请自行异步化）
 * @param intervalMs 本订阅者的刷新周期，默认 1000ms
 */
export function useSharedTick(run: () => void, intervalMs = 1000): UseSharedTickReturn {
  const polling = usePolling(run, intervalMs, {
    autoStart: false,
    backoff: false,
    // 展示刷新不涉及网络：被 KeepAlive 缓存时照旧走表，与收口前的行为一致
    pauseWhenDeactivated: false,
    onError: (e) => logError('tick:callback', e),
  })

  // 生命周期由本模块自己接管：首轮等待满一个周期（start(false)），
  // 不沿用 usePolling 的 autoStart（后者会挂载即执行一次）
  onMounted(() => polling.start(false))
  onUnmounted(() => polling.stop())

  return {
    start: () => polling.start(false),
    stop: () => polling.stop(),
    setInterval: polling.setInterval,
    active: () => polling.running.value,
  }
}

/**
 * 测试用：清空订阅并停表，等价于「所有组件都已卸载」。
 *
 * 调度器已合并到 usePolling，这里只做转发，保证既有测试/全局 setup 的调用点不变。
 */
export function __resetSharedTickForTests(): void {
  __resetUsePollingForTests()
}
