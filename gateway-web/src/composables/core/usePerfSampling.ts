import { computed, onUnmounted, ref } from 'vue'
import { usePolling } from './usePolling'

/** 环形队列容量：每秒采一帧，保留最近 30 秒 */
const PERF_HISTORY_CAP = 30
const SAMPLE_INTERVAL_MS = 1000

/**
 * 请求监视器面板的性能采样：FPS + JS Heap 使用率。
 *
 * 浏览器安全限制下没有真正的 CPU 使用率 API，因此用 FPS 反映主线程是否被
 * JS 阻塞（低于 30fps 说明有长任务），用 JS Heap 使用率反映 GC 压力。
 * `performance.memory` 是 Chrome 私有 API，通过特性检测优雅降级为 null。
 *
 * 采样开销虽小但没必要常驻，因此**不在挂载时自动启动**：由调用方在面板打开
 * （或开发环境常驻）时调 `startPerfSampling`，关闭时调 `stopPerfSampling`，
 * 卸载时自动停止（见 `onUnmounted`）。
 *
 * 内存采样走共享调度器 `usePolling`（W2-#12 定时器收敛），采样在本地、不发请求，
 * 故关闭失败退避；标签页隐藏时照旧采样。
 */
export function usePerfSampling() {
  /** FPS 环形队列，单位：帧/秒 */
  const fpsHistory = ref<number[]>([])
  /** 当前 FPS（0 表示未采样/采样中） */
  const currentFps = ref(0)
  /** JS Heap 使用率环形队列，单位：百分比 */
  const memHistory = ref<number[]>([])
  /** 当前内存使用率百分比（null 表示浏览器不支持 performance.memory） */
  const currentMemPct = ref<number | null>(null)

  let rafId: number | null = null
  let frameCount = 0
  let lastFpsTick = 0
  let active = false

  function hasMemoryAPI(): boolean {
    return typeof performance !== 'undefined' && 'memory' in performance
  }

  /** 采集一帧 JS Heap 使用率（未启动采样或浏览器不支持时直接返回） */
  function sampleMemory(): void {
    if (!active || !hasMemoryAPI()) return
    const mem = (performance as unknown as {
      memory: { usedJSHeapSize: number; totalJSHeapSize: number }
    }).memory
    if (mem.totalJSHeapSize > 0) {
      const pct = Math.round((mem.usedJSHeapSize / mem.totalJSHeapSize) * 100)
      currentMemPct.value = pct
      memHistory.value.push(pct)
      if (memHistory.value.length > PERF_HISTORY_CAP) memHistory.value.shift()
    }
  }

  const memPoller = usePolling(sampleMemory, SAMPLE_INTERVAL_MS, {
    autoStart: false,
    backoff: false,
    pauseWhenHidden: false,
  })

  function startPerfSampling() {
    if (active) return
    active = true
    frameCount = 0
    lastFpsTick = performance.now()
    currentFps.value = 0

    const tick = () => {
      if (!active) return
      frameCount++
      const now = performance.now()
      if (now - lastFpsTick >= SAMPLE_INTERVAL_MS) {
        const interval = now - lastFpsTick
        const fps = Math.round((frameCount * 1000) / interval)
        currentFps.value = fps
        fpsHistory.value.push(fps)
        if (fpsHistory.value.length > PERF_HISTORY_CAP) fpsHistory.value.shift()
        frameCount = 0
        lastFpsTick = now
      }
      rafId = requestAnimationFrame(tick)
    }
    rafId = requestAnimationFrame(tick)

    if (hasMemoryAPI()) {
      sampleMemory()
      memPoller.start()
    } else {
      currentMemPct.value = null
    }
  }

  function stopPerfSampling() {
    if (!active) return
    active = false
    if (rafId !== null) {
      cancelAnimationFrame(rafId)
      rafId = null
    }
    memPoller.stop()
  }

  onUnmounted(stopPerfSampling)

  /** FPS 健康度：>= 50 正常（绿），30-49 警告（黄），< 30 危险（红） */
  const fpsTagType = computed<'success' | 'warning' | 'danger' | 'info'>(() => {
    if (currentFps.value === 0) return 'info'
    if (currentFps.value >= 50) return 'success'
    if (currentFps.value >= 30) return 'warning'
    return 'danger'
  })

  /** 内存健康度：< 70% 正常，70-85% 警告，>= 85% 危险 */
  const memTagType = computed<'success' | 'warning' | 'danger' | 'info'>(() => {
    if (currentMemPct.value === null) return 'info'
    if (currentMemPct.value < 70) return 'success'
    if (currentMemPct.value < 85) return 'warning'
    return 'danger'
  })

  return {
    fpsHistory,
    currentFps,
    memHistory,
    currentMemPct,
    startPerfSampling,
    stopPerfSampling,
    fpsTagType,
    memTagType,
  }
}
