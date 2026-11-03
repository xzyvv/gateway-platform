<template>
  <div class="busload">
    <div class="busload-toolbar">
      <el-button size="small" @click="refresh"><el-icon><Refresh /></el-icon> 刷新</el-button>
    </div>

    <div v-for="bus in loads" :key="bus.bus" class="bus-card">
      <div class="bus-card-head">
        <div class="bus-info">
          <el-icon><Connection /></el-icon>
          <span class="bus-name">{{ bus.bus }}</span>
          <el-tag size="small" type="primary" effect="plain">{{ bus.type }}</el-tag>
          <el-tag
            v-if="isIdle(bus)"
            size="small"
            type="info"
            effect="plain"
            title="窗口内收发速率为 0，错误率/延迟没有分母，不做猜测"
          >无采样</el-tag>
        </div>
        <div class="bus-head-right">
          <div class="utilization" :class="utilClass(bus.utilization)">
            <el-icon><Odometer /></el-icon>
            使用率 {{ fmtPct(bus.utilization, 1) }}
          </div>
          <!-- 单总线下钻：只重取这一条（GET /bus-load/:bus），避免全局刷新打断其它总线的曲线 -->
          <el-button
            class="bus-refresh"
            size="small"
            link
            :loading="refreshingBus === bus.bus"
            @click="refreshOne(bus.bus)"
          >刷新</el-button>
        </div>
      </div>

      <div class="bus-metrics">
        <div class="metric">
          <div class="metric-label">发送速率</div>
          <div class="metric-value">{{ fmt(bus.txRate, 1) }} <span>包/s</span></div>
        </div>
        <div class="metric">
          <div class="metric-label">接收速率</div>
          <div class="metric-value">{{ fmt(bus.rxRate, 1) }} <span>包/s</span></div>
        </div>
        <div class="metric">
          <div class="metric-label">错误率</div>
          <div
            class="metric-value"
            :class="{ 'text-warning': !isIdle(bus) && (bus.collisionRate ?? 0) > 1 }"
          >{{ isIdle(bus) ? '—' : fmtPct(bus.collisionRate, 2) }}</div>
        </div>
        <div class="metric">
          <div class="metric-label">平均延迟</div>
          <div class="metric-value">
            {{ isIdle(bus) ? '—' : fmt(bus.avgLatency, 0) }}
            <span v-if="!isIdle(bus)">ms</span>
          </div>
        </div>
        <div class="metric" title="队列为全局单队列，后端无法按总线拆分，故不提供">
          <div class="metric-label">数据积压</div>
          <div class="metric-value">{{ bus.backlog == null ? '—' : bus.backlog }}</div>
        </div>
      </div>

      <div class="chart-wrap">
        <div
          :ref="el => setChartRef(el, bus.bus)"
          class="chart"
          :data-chart-label="`总线 ${bus.bus} 收发负载趋势图`"
        ></div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick, type ComponentPublicInstance } from 'vue'
import { usePolling } from '@/composables/core/usePolling'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import {Refresh, Connection, Odometer} from '@element-plus/icons-vue'
import echarts, { initChart, disposeChart } from '@/utils/echarts'
import { tooltipStyle, defaultGrid, valueAxisStyle, categoryAxisStyle, legendStyle, axisNameStyle } from '@/utils/chartStyle'
import { CHART_PRIMARY, CHART_SUCCESS } from '@/utils/chartTheme'
import { formatTimeOfDay } from '@/utils/datetime'
import { listBusLoadsView as listBusLoads, refreshBusLoadsView as refreshBusLoads, getBusLoadView as getBusLoad, type BusLoadView as BusLoad } from '@/api/diagnostics'

const loads = ref<BusLoad[]>([])
const chartRefs = new Map<string, HTMLElement>()
const chartInstances = new Map<string, echarts.ECharts>()

function setChartRef(el: Element | ComponentPublicInstance | null, bus: string) {
  if (el instanceof HTMLElement) chartRefs.set(bus, el)
}

/** 数值格式化：null（无基线/区间无采集）渲染「—」，div 不伪造 0 */
function fmt(v: number | null, digits: number): string {
  return v == null ? '—' : v.toFixed(digits)
}

/** 百分比格式化：null 渲染单个「—」，避免出现「—%」 */
function fmtPct(v: number | null, digits: number): string {
  return v == null ? '—' : `${v.toFixed(digits)}%`
}

function utilClass(u: number | null) {
  if (u == null) return 'unknown'
  if (u >= 80) return 'critical'
  if (u >= 60) return 'warning'
  return 'normal'
}

/**
 * 无采样判定：窗口内收发速率都低于显示精度（表格按 0.1 包/s 渲染，<0.05 会显示 0.0）。
 * 实测会出现「使用率 6.2% + 错误率 100% + 速率 0.0 包/s」的自相矛盾组合——
 * 分母只剩个位数包时 100% 不具统计意义，用户也分不清是链路全错还是根本没收发。
 * 此时错误率与延迟统一降级为「—」，并在卡头挂一个「无采样」标签说明原因。
 */
function isIdle(bus: BusLoad): boolean {
  return (bus.txRate ?? 0) < 0.05 && (bus.rxRate ?? 0) < 0.05
}

/**
 * 图表 option 骨架：颜色/网格/tooltip/坐标轴/series 类型等静态部分只构建一次。
 *
 * 3s 轮询只更新 `xAxis.data` 与 `series[].data`（`notMerge: false` 局部合并），
 * 不再每次全量重建 option——原实现对每张图每 3s 做一次全量 setOption，
 * 数据没变时也在空转重绘。
 */
function createBaseOption() {
  return {
    grid: defaultGrid,
    tooltip: tooltipStyle,
    legend: { ...legendStyle, data: ['发送', '接收'] },
    xAxis: { ...categoryAxisStyle, data: [] as string[] },
    yAxis: { ...valueAxisStyle, name: '包/s', nameTextStyle: axisNameStyle },
    series: [
      {
        name: '发送',
        type: 'line' as const,
        data: [] as number[],
        smooth: true,
        lineStyle: { color: CHART_PRIMARY },
        areaStyle: { opacity: 0.1 },
      },
      {
        name: '接收',
        type: 'line' as const,
        data: [] as number[],
        smooth: true,
        lineStyle: { color: CHART_SUCCESS },
        areaStyle: { opacity: 0.1 },
      },
    ],
  }
}

/** 每张图表最近一次渲染的数据指纹：数据未变化时直接跳过 setOption */
const chartFingerprints = new Map<string, string>()

function renderChart(bus: BusLoad) {
  const el = chartRefs.get(bus.bus)
  if (!el) return
  let chart = chartInstances.get(bus.bus)
  const times = bus.history.map((h) => formatTimeOfDay(h.time))
  const tx = bus.history.map((h) => h.tx)
  const rx = bus.history.map((h) => h.rx)
  // 指纹覆盖坐标标签与两条曲线：窗口滑动（长度/时间变化）或数值更新都会命中
  const fingerprint = JSON.stringify([times, tx, rx])
  if (chart) {
    if (chartFingerprints.get(bus.bus) === fingerprint) return
  } else {
    chart = initChart(el)
    chartInstances.set(bus.bus, chart)
    // 骨架只注入一次；后续轮询走下面的局部合并
    chart.setOption(createBaseOption())
  }
  chartFingerprints.set(bus.bus, fingerprint)
  chart.setOption({ xAxis: { data: times }, series: [{ data: tx }, { data: rx }] }, { notMerge: false })
}

async function load() {
  try {
  loads.value = await listBusLoads()
  await nextTick()
  loads.value.forEach((b) => renderChart(b))
  } catch (e) { showToast(e, '加载总线负载失败') }
}

async function refresh() {
  try {
  loads.value = await refreshBusLoads()
  // 总线集合可能变化（新增总线需要先完成 DOM 挂载，才能拿到容器 ref）
  await nextTick()
  loads.value.forEach((b) => renderChart(b))
  } catch (e) { showToast(e, '刷新失败') }
}

/** 正在单条刷新的总线名（null 表示无） */
const refreshingBus = ref<string | null>(null)

/**
 * 单总线下钻：只请求这一条总线（`GET /api/diagnostics/bus-load/:bus`），
 * 用它替换列表里的对应卡片，其它总线的曲线与轮询节奏不受影响。
 */
async function refreshOne(bus: string) {
  refreshingBus.value = bus
  try {
    const latest = await getBusLoad(bus)
    const idx = loads.value.findIndex((b) => b.bus === bus)
    if (idx >= 0) loads.value[idx] = latest
    await nextTick()
    renderChart(latest)
  } catch (e) {
    showToast(e, `加载总线 ${bus} 负载失败`)
  } finally {
    refreshingBus.value = null
  }
}

// 常驻轮询：usePolling 统一提供隐藏暂停 / 失败退避 / 卸载清理。
// 首屏用 load（listBusLoads），轮询用 refresh（refreshBusLoads 触发后端主动采样），两者不同接口。
const { start: startPoll } = usePolling(refresh, 3000, { autoStart: false })

onMounted(async () => {
  await load()
  startPoll()
})

onUnmounted(() => {
  chartInstances.forEach((c) => disposeChart(c))
  chartInstances.clear()
  chartFingerprints.clear()
})
</script>

<style scoped>
.busload { padding: 0px; }
.busload-toolbar { display: flex; justify-content: flex-end; margin-bottom: 12px; }
.bus-card { background: var(--bg-card); border-radius: var(--radius-card); padding: 16px; margin-bottom: 16px; border: 1px solid var(--border-light); }
.bus-card-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 16px; }
.bus-info { display: flex; align-items: center; gap: 8px; }
.bus-head-right { display: flex; align-items: center; gap: 12px; }
.bus-name { font-weight: 600; font-family: monospace; }
.utilization { display: flex; align-items: center; gap: 4px; font-weight: 600; }
.utilization.normal { color: var(--text-success); }
.utilization.warning { color: var(--text-warning); }
.utilization.critical { color: var(--text-danger); }
.utilization.unknown { color: var(--text-secondary); font-weight: 400; }
.bus-metrics { display: grid; grid-template-columns: repeat(5, 1fr); gap: 12px; margin-bottom: 16px; }
.metric { text-align: center; padding: 10px; background: var(--bg-fill); border-radius: var(--radius-table); }
.metric-label { font-size: 0.75rem; color: var(--text-secondary); margin-bottom: 4px; }
.metric-value { font-size: 1.125rem; font-weight: 700; color: var(--text-primary); }
.metric-value span { font-size: 0.6875rem; color: var(--text-secondary); font-weight: 400; }
.text-warning { color: var(--text-warning) !important; }
.text-danger { color: var(--text-danger) !important; }
.chart-wrap { border-top: 1px solid var(--border-light); padding-top: 12px; }
.chart { height: 180px; }
</style>
