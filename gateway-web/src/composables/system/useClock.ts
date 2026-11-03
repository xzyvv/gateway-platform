import { onMounted, ref } from 'vue'
import { formatClock } from '@/utils/datetime'
import { useSharedTick } from '@/composables/core/useSharedTick'

/**
 * 全局实时时钟（1s 粒度）
 *
 * 供 Layout 顶栏（formatClock）与系统总览页（formatDateTime）共用——此前两个
 * composable 各自维护 setInterval(1000) 时钟，逻辑重复（视觉审计 P2-7 去重）。
 *
 * - `now`：格式化时间字符串，格式由调用方注入（默认顶栏 formatClock 口径）
 * - `nowTs`：数值时间戳，与 now 同步刷新，供"数据停留在多少秒前"这类相对
 *   时间计算使用（如 useLayoutClock.staleSeconds）
 *
 * 定时器委托 useSharedTick（P0）：顶栏、系统总览、Dashboard 的"x 秒前"文案
 * 共用同一个 setInterval，标签页隐藏时整体停表。组件卸载后自动摘除订阅；
 * `__clockTimerActive` 供单测断言订阅已释放。
 */
export function useClock(format: (ts: number) => string = () => formatClock()) {
  const now = ref('')
  const nowTs = ref(Date.now())

  function updateClock() {
    nowTs.value = Date.now()
    now.value = format(nowTs.value)
  }

  const tick = useSharedTick(updateClock, 1000)

  // 首屏立即有值，不等第一个 tick
  onMounted(updateClock)

  return {
    now,
    nowTs,
    /** 测试钩子：本时钟是否仍在共享 tick 中登记 */
    __clockTimerActive: () => tick.active(),
  }
}
