import { ref, shallowRef, triggerRef, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { ElMessage } from 'element-plus'
import { GatewayWebSocket, acquireSharedWebSocket, type SharedWebSocketHandle } from '@/utils/websocket'
import { storeToRefs } from 'pinia'
import { useWebsocketStore } from '@/stores/websocket'
import { useFavorites } from '@/composables/core/useFavorites'
import { usePolling } from '@/composables/core/usePolling'
import { downloadCsv } from '@/utils/exportCsv'
import type { DataPoint } from '@/api/types'
import { getStatus } from '@/api/status'
// 展示格式化纯函数（W3b-13 从本文件拆出，便于单独覆盖）
import { formatValue, formatTime, qualityClass, qualityTagType, qualityLabel, typeLabel } from '@/utils/realtimeFormat'
// 图表与视口分配（W3b-13 拆出：本文件只留「连接 + 表格状态 + 空态」编排）
import { useRealtimeChart } from './useRealtimeChart'
import { useRealtimeViewport } from './useRealtimeViewport'

type RealtimePoint = DataPoint & {
  _flash?: boolean
  /** 突变方向：单帧相对变化超过阈值时标记，值再次变化时重算 */
  _surge?: 'up' | 'down'
  /** 突变幅度（百分比，带符号），供 tooltip 展示 */
  _surgePct?: number
}

/** 突变阈值：单帧相对变化 ≥50% 视为突变（基数过小视为抖动，不标记） */
const SURGE_THRESHOLD_PCT = 50
const SURGE_MIN_BASE = 1e-6
/** P1: 已连接且已配置点位，但连续这么久没有任何帧 → 判定为采集停滞（而非「还在等首帧」） */
const STALL_MS = 10_000
/** 运行态点位总数刷新间隔（配置保存后点位会增减，但没必要为此建立推送） */
const STATUS_POLL_MS = 5_000

/**
 * 突变检测：相邻两帧数值型数据的相对变化。
 * 非数值（枚举字符串 ON/OPEN 等）与 0 附近抖动不标记；返回值带符号百分比。
 */
function detectSurge(oldV: unknown, newV: unknown): { dir: 'up' | 'down'; pct: number } | undefined {
  const a = typeof oldV === 'number' ? oldV : Number(oldV)
  const b = typeof newV === 'number' ? newV : Number(newV)
  if (!Number.isFinite(a) || !Number.isFinite(b) || a === b) return undefined
  const base = Math.abs(a)
  if (base < SURGE_MIN_BASE) return undefined
  const pct = ((b - a) / base) * 100
  if (Math.abs(pct) < SURGE_THRESHOLD_PCT) return undefined
  return { dir: pct > 0 ? 'up' : 'down', pct: Math.round(pct) }
}

/**
 * Realtime 页面数据层封装
 *
 * 负责三件事：订阅共享 WebSocket 并写入表格状态、空态分区、采集停滞判定。
 * 图表（实例 / 节流重绘 / 历史缓冲）在 `useRealtimeChart`，视口高度分配在
 * `useRealtimeViewport`，展示格式化在 `utils/realtimeFormat`。
 */
export function useRealtime() {
  const { isFavorite, toggleFavorite } = useFavorites()
  const onlyFavorites = ref(false)

  // 冻结（暂停刷新）：冻结期间推送仅缓存最新帧+续写历史，表格/图表不渲染；
  // 恢复时回放最新帧，图表借 historyMap 把冻结段补全。
  const paused = ref(false)
  const pendingLatest = new Map<string, DataPoint>()

  const wsStore = useWebsocketStore()
  const { isConnected: connected, reconnectCount: reconnectAttempts } = storeToRefs(wsStore)

  const { tableHeight, chartHeight, refreshViewport } = useRealtimeViewport()
  // shallowRef：RealtimePoint 是扁平结构（无嵌套对象），写入时整体替换元素
  // （`pointList.value[idx] = {...dp}`），从不在原地改字段，故不需要深层代理。
  // 1200 点高频推送下，深层 ref 的代理开销与渲染成本同量级。
  const pointList = shallowRef<RealtimePoint[]>([])
  // W17: 点位索引 Map，避免每条推送都 findIndex 全表扫描（O(N²) → O(N)）。
  const pointIndex = new Map<string, number>()

  const chart = useRealtimeChart({
    getTopPoints: () => pointList.value.slice(0, 5),
    refreshViewport,
  })

  // P1: 区分「没配点位」与「配了但采不到」。只用 WS 有没有收到帧无法判断——
  // 两者表现都是「空表 + 等待」，用户会一直等一个永远不会来的数据。
  //  - configuredPoints：运行态点位总数（来自 /api/status 的 total_points），null = 未知
  //  - stalled：已连、已配置，但超过 STALL_MS 仍无任何帧 → 采集异常
  const configuredPoints = ref<number | null>(null)
  const stalled = ref(false)

  const displayPoints = computed(() => {
    // 排序规则只有「收藏优先」一条，本质是二值稳定分区：
    // 旧实现每条推送都 `[...list].sort()`，1200 点位 × 1Hz 即每秒一次 O(N log N)
    // 比较 + 整数组拷贝；这里改成一次 O(N) 遍历分桶，顺序与稳定排序完全一致。
    const fav: RealtimePoint[] = []
    const rest: RealtimePoint[] = []
    for (const p of pointList.value) {
      const isFav = isFavorite(p.point_id)
      if (onlyFavorites.value && !isFav) continue
      if (isFav) fav.push(p)
      else rest.push(p)
    }
    // 无收藏时 rest 本身即结果，省掉一次 concat 分配
    return fav.length ? fav.concat(rest) : rest
  })

  /**
   * N4: 空态分区。多种「空」的原因完全不同，共用一句话会误导：
   *  - filtered：其实有实时数据，是「仅看收藏」把表过滤空了
   *  - disconnected：WS 没连上，需要重连
   *  - unconfigured：压根没配点位（total_points=0），等多久都不会有数据
   *  - stalled：已配置点位但长时间收不到帧，属于采集异常
   *  - waiting：已连上、首帧还没到，属于正常等待
   */
  const emptyKind = computed<'filtered' | 'disconnected' | 'unconfigured' | 'stalled' | 'waiting'>(() => {
    if (pointList.value.length > 0) return 'filtered'
    if (!connected.value) return 'disconnected'
    if (configuredPoints.value === 0) return 'unconfigured'
    if (stalled.value) return 'stalled'
    return 'waiting'
  })

  const emptyText = computed(() => {
    if (emptyKind.value === 'filtered') return '当前筛选条件下没有点位'
    if (emptyKind.value === 'unconfigured') return '尚未配置任何采集点位，不会产生实时数据'
    if (emptyKind.value === 'stalled') {
      const n = configuredPoints.value
      return `已配置 ${n ?? '若干'} 个点位（${STALL_MS / 1000} 秒内未收到任何数据），采集可能异常`
    }
    if (emptyKind.value === 'waiting') return '已连接，等待首帧采集数据…'
    if (reconnectAttempts.value > 0) return `连接已断开，正在第 ${reconnectAttempts.value} 次重连...`
    return 'WebSocket 未连接，请检查网络或重新登录'
  })

  /** 空态主行动：只有「能立刻解决问题」时才给按钮，纯等待态不给假入口 */
  const emptyActionText = computed(() => {
    if (emptyKind.value === 'filtered') return '显示全部点位'
    if (emptyKind.value === 'unconfigured') return '去配置点位'
    if (emptyKind.value === 'stalled') return '去连接测试'
    if (emptyKind.value === 'disconnected' && reconnectAttempts.value > 0) return '立即重连'
    return ''
  })

  /** 空态行动的跳转目标（内部路由）；为 null 表示由调用方处理（取消筛选/重连） */
  const emptyActionRoute = computed<string | null>(() => {
    if (emptyKind.value === 'unconfigured') return '/points?tab=points'
    if (emptyKind.value === 'stalled') return '/ops?tab=connection-test'
    return null
  })

  function reconnectNow() {
    if (!ws) return
    void ws.connect()
  }

  /**
   * 冻结/恢复切换。恢复时回放冻结期间各点位最新一帧（表格立即追平），
   * 并强制 structure 重建——冻结段已写入 historyMap，全量重建让图表补全缺口。
   */
  function togglePause() {
    paused.value = !paused.value
    if (paused.value || !pendingLatest.size) return
    const batch = Array.from(pendingLatest.values())
    pendingLatest.clear()
    // 恢复：回放最新帧，图表在下一个节流窗口从 historyMap 全量重绘（冻结段自然补全）
    onMessage(batch)
  }

  function exportCsv() {
    const rows = displayPoints.value
    if (!rows.length) return
    downloadCsv(rows)
    ElMessage.success(`已导出 ${rows.length} 条数据`)
  }

  // N8: chartHeight 变化由 DOM 样式驱动，需等这一帧渲染完成后再 resize，否则图表按旧尺寸绘制
  watch(chartHeight, () => {
    void nextTick(() => chart.resizeChart())
  })

  let ws: GatewayWebSocket | null = null
  // ROI #3：共享连接句柄 + 消息退订函数（避免同一端点重复建连）
  let wsHandle: SharedWebSocketHandle | null = null
  let wsUnsubMessage: (() => void) | null = null
  let flashClearTimer: ReturnType<typeof setTimeout> | null = null
  let unmounted = false

  function onMessage(data: DataPoint[]) {
    if (unmounted) return
    if (!Array.isArray(data)) return
    // P1: 收到帧即证明采集正常，清掉「停滞」标记并重置计时基准
    if (data.length > 0) {
      lastFrameAt = Date.now()
      stalled.value = false
    }
    if (paused.value) {
      // 冻结分支：不触发表格/图表渲染，但历史必须续写——恢复后图表才能补全冻结段
      // （刻意不调 markHistoryDirty / scheduleFlush，跨度标签与图表也要停住）
      for (const dp of data) {
        if (!dp || !dp.point_id) continue
        pendingLatest.set(dp.point_id, dp)
        chart.appendHistory(dp)
      }
      return
    }
    for (const dp of data) {
      if (!dp || !dp.point_id) continue
      const idx = pointIndex.get(dp.point_id)
      if (idx !== undefined && idx < pointList.value.length) {
        const old = pointList.value[idx]!
        const flash = old.value !== dp.value || old.quality !== dp.quality
        // 值未变化时保留旧突变标记（突变后数值停住，标记不应闪一下就消失）；
        // 值变化时重算，回归正常波动则清除
        const surge =
          old.value === dp.value
            ? old._surge
              ? { dir: old._surge, pct: old._surgePct ?? 0 }
              : undefined
            : detectSurge(old.value, dp.value)
        pointList.value[idx] = { ...dp, _flash: flash, _surge: surge?.dir, _surgePct: surge?.pct }
      } else {
        const newIdx = pointList.value.length
        pointList.value.push({ ...dp, _flash: true })
        pointIndex.set(dp.point_id, newIdx)
      }
      chart.appendHistory(dp)
    }
    // shallowRef 的数组下标赋值/push 不触发依赖，需显式通知一次（与 flushLogs 同理）
    triggerRef(pointList)
    // 通知 chartTimeSpan 重算（historyMap 非响应式，需显式信号）
    chart.markHistoryDirty()
    if (flashClearTimer) clearTimeout(flashClearTimer)
    flashClearTimer = setTimeout(() => {
      flashClearTimer = null
      if (unmounted) return
      const next = pointList.value.slice()
      let mutated = false
      for (let i = 0; i < next.length; i++) {
        if (next[i]!._flash) {
          next[i] = { ...next[i]!, _flash: false }
          mutated = true
        }
      }
      if (mutated) pointList.value = next
    }, 800)

    // 图表数据源是 historyMap，统一在节流窗口里从最终态重绘；
    // 不再需要「top5 集合变化 → 结构重建」的两段式判断。
    chart.scheduleFlush()
  }

  // P1: 运行态点位总数轮询 + 采集停滞判定。
  // 状态接口失败不阻塞实时页，保持 configuredPoints=null（此时空态回落为「等待首帧」）。
  let connectedAt = 0
  let lastFrameAt = 0

  async function refreshStatus(): Promise<void> {
    try {
      const s = await getStatus()
      const n = Number(s.total_points)
      configuredPoints.value = Number.isFinite(n) ? n : null
    } catch {
      /* 忽略：状态未知时按「等待首帧」处理，不干扰实时数据 */
    }
  }

  function tickStall(): void {
    if (unmounted) return
    // 有数据 → 一定不是「空态」，无需判定
    if (pointList.value.length > 0) {
      stalled.value = false
      return
    }
    if (!connected.value) {
      stalled.value = false
      connectedAt = 0
      return
    }
    // 未知或确实没配点位：交给 unconfigured/waiting 分区，不报「异常」
    if (configuredPoints.value === null || configuredPoints.value === 0) {
      stalled.value = false
      return
    }
    if (!connectedAt) connectedAt = Date.now()
    const since = lastFrameAt || connectedAt
    stalled.value = Date.now() - since >= STALL_MS
  }

  // 状态轮询统一走共享调度器（W2-#12）：标签页隐藏自动暂停、组件卸载自动摘除，
  // 不再本页手挂 setInterval。refreshStatus 内部已吞掉失败，故关闭失败退避。
  const statusPoller = usePolling(
    () => {
      void refreshStatus()
      tickStall()
    },
    STATUS_POLL_MS,
    { autoStart: false, backoff: false },
  )

  onMounted(async () => {
    await nextTick()
    chart.mountChart()
    wsStore.subscribe()
    // ROI #3：与 Monitor.vue 共享同一条 /api/realtime 连接（按 path 引用计数），
    // 不再各自 new 一条 → 消除同一端点的重复建连。
    wsHandle = acquireSharedWebSocket('/api/realtime')
    ws = wsHandle.ws
    // 用多播监听而非 onMessage 单槽：共享实例上 Monitor 也会注册自己的回调，
    // 单槽会互相覆盖（谁后注册谁生效）。
    wsUnsubMessage = ws.addMessageListener(onMessage)
    // 首屏状态走一次同步请求，之后交给共享调度器按周期补
    void refreshStatus()
    statusPoller.start()
  })

  onUnmounted(() => {
    unmounted = true
    wsStore.unsubscribe()
    wsUnsubMessage?.()
    wsUnsubMessage = null
    // 释放共享引用；仅当没有其它订阅者时才真正关闭底层连接
    wsHandle?.release()
    wsHandle = null
    ws = null
    if (flashClearTimer) {
      clearTimeout(flashClearTimer)
      flashClearTimer = null
    }
    chart.dispose()
    pendingLatest.clear()
    pointIndex.clear()
  })

  return {
    // state
    pointList, connected, reconnectAttempts, onlyFavorites, paused, configuredPoints, stalled, chartError: chart.chartError,
    displayPoints, emptyText, emptyKind, emptyActionText, emptyActionRoute, tableHeight, chartHeight, chartRef: chart.chartRef, chartTimeSpan: chart.chartTimeSpan,
    // helpers
    isFavorite, toggleFavorite,
    qualityClass, qualityTagType, qualityLabel, typeLabel,
    formatValue, formatTime,
    // actions
    reconnectNow, exportCsv, togglePause,
  }
}
