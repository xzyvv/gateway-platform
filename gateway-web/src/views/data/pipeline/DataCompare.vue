<template>
  <div class="compare">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="选择要对比的点位（最多 4 个）" icon="Connection" />
      </template>
      <el-select
        v-model="selectedPointIds"
        multiple
        collapse-tags
        collapse-tags-tooltip
        aria-label="选择要对比的点位"
        :multiple-limit="4"
        placeholder="选择要对比的点位"
        style="width: 100%"
        filterable
      >
        <el-option v-for="p in allPoints" :key="p.id" :label="`${p.deviceName} / ${p.pointName}`" :value="p.id" />
      </el-select>
      <div class="time-range">
        <el-radio-group v-model="timeRange" size="small">
          <el-radio-button value="1">最近 1 小时</el-radio-button>
          <el-radio-button value="6">最近 6 小时</el-radio-button>
          <el-radio-button value="24">最近 24 小时</el-radio-button>
          <el-radio-button value="168">最近 7 天</el-radio-button>
        </el-radio-group>
        <el-button type="primary" size="small" :disabled="selectedPointIds.length === 0" @click="loadData">
          <el-icon><Search /></el-icon> 查询
        </el-button>
        <span v-if="selectedPointIds.length === 0" class="query-hint">请先选择点位</span>
      </div>
    </el-card>

    <el-card v-if="selectedPointIds.length > 0" shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="对比点位" icon="DataLine">
          <template #extra>
            <span class="selected-count">已选 {{ selectedPointIds.length }} 个点位</span>
          </template>
        </SectionHeader>
      </template>
      <div ref="chartEl" class="compare-chart" data-chart-label="两组数据对比趋势图"></div>
    </el-card>

    <el-card v-if="dataRows.length" shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="对比表格" icon="Document" />
      </template>
      <DataTable :data="dataRows" :columns="dataColumns" row-key="time" stripe empty-text="暂无对比数据" :max-height="300" />
    </el-card>

    <el-card v-if="stats.length" shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="统计摘要" icon="DataAnalysis" />
      </template>
      <DataTable :data="stats" :columns="statColumns" row-key="name" stripe empty-text="暂无统计" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import {ref, computed, onMounted, onUnmounted, nextTick, h} from 'vue'
import {Search} from '@element-plus/icons-vue'
import echarts, { initChart, disposeChart } from '@/utils/echarts'
import { tooltipStyle, defaultGrid, valueAxisStyle, categoryAxisStyle, legendStyle } from '@/utils/chartStyle'
import { fetchHistorySamples } from '@/composables/data/useHistoryQuery'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import type { ColumnDef } from '@/components/DataTable.vue'
import { formatTimeOfDay } from '@/utils/datetime'
import { CHART_PRIMARY, CHART_SUCCESS, CHART_WARNING, CHART_DANGER } from '@/utils/chartTheme'

interface PointOption { id: string; pointName: string; deviceName: string; color: string }

// 多点位对比的序列色：语义固定（首色主蓝、绿/琥珀/红按序），色值统一取 chartTheme
const colors = [CHART_PRIMARY, CHART_SUCCESS, CHART_WARNING, CHART_DANGER]
const allPoints = ref<PointOption[]>([])
const selectedPointIds = ref<string[]>([])
const timeRange = ref('24')
const chartEl = ref<HTMLElement>()
let chart: echarts.ECharts | null = null

const selectedPoints = computed(() =>
  selectedPointIds.value.map((id, i) => {
    const p = allPoints.value.find((x) => x.id === id)
    return p ? { ...p, color: colors[i] } : null
  }).filter(Boolean) as (PointOption & { color: string })[]
)

const dataRows = ref<Record<string, string | number>[]>([])
type StatRow = { name: string; min: number; max: number; avg: number; range: number; std: number }
const stats = ref<StatRow[]>([])

// 表格统一走 DataTable；对比列随所选点位动态生成
const dataColumns = computed<ColumnDef<Record<string, string | number>>[]>(() => [
  { key: 'time', title: '时间', width: 170, fixed: true },
  ...selectedPoints.value.map((p) => ({
    key: p.id,
    title: p.pointName,
    minWidth: 120,
    cell: (row: Record<string, string | number>) =>
      h('span', { style: { color: p.color, fontWeight: 600 } }, String(row[p.id] ?? '无')),
  })),
])

const statColumns: ColumnDef<StatRow>[] = [
  { key: 'name', title: '点位', width: 160 },
  { key: 'min', title: '最小值', width: 100 },
  { key: 'max', title: '最大值', width: 100 },
  { key: 'avg', title: '平均值', width: 100 },
  { key: 'range', title: '极差', width: 100 },
  { key: 'std', title: '标准差', width: 100 },
]

async function loadData() {
  if (selectedPointIds.value.length === 0) return
  const hours = parseInt(timeRange.value)
  const now = Date.now()
  const start = now - hours * 3600000

  const series: { name: string; data: number[]; color: string }[] = []
  const rows: Record<string, string | number>[] = []
  const statsList: typeof stats.value = []

  // Fetch real history data for each selected point
  const results = await Promise.all(
    selectedPoints.value.map(async (p) => {
      try {
        // P2-1：返回体为信封内的 { items, total, ... } 而非裸数组
        // W1：不再视图内手拼 query 串，统一走 `/api/history` 的唯一取数入口
        const res = await fetchHistorySamples({ pointId: p.id, start, end: now })
        return { point: p, data: res?.items ?? [] }
      } catch { return { point: p, data: [] as { timestamp: number; value: number }[] } }
    })
  )

  // Build unified time axis from all points' timestamps
  const allTs = Array.from(new Set(results.flatMap((r) => r.data.map((d) => d.timestamp)))).sort((a, b) => a - b)
  const times = allTs.map((ts) => formatTimeOfDay(ts))

  for (const r of results) {
    const valMap = new Map(r.data.map((d) => [d.timestamp, d.value]))
    const data = allTs.map((ts) => valMap.get(ts) ?? null)
    const validData = data.filter((v): v is number => v !== null)
    series.push({ name: r.point.pointName, data: data as number[], color: r.point.color })
    if (validData.length > 0) {
      const min = Math.min(...validData)
      const max = Math.max(...validData)
      const avg = validData.reduce((s, v) => s + v, 0) / validData.length
      const std = Math.sqrt(validData.reduce((s, v) => s + (v - avg) ** 2, 0) / validData.length)
      statsList.push({ name: `${r.point.deviceName}/${r.point.pointName}`, min, max, avg: Math.round(avg * 100) / 100, range: Math.round((max - min) * 100) / 100, std: Math.round(std * 100) / 100 })
    }
  }

  for (let i = 0; i < allTs.length; i++) {
    const row: Record<string, string | number> = { time: times[i]! }
    for (let j = 0; j < results.length; j++) {
      const sp = selectedPoints.value[j]!
      const sd = series[j]!.data
      row[sp.id] = sd[i] ?? '-'
    }
    rows.push(row)
  }

  dataRows.value = rows
  stats.value = statsList

  await nextTick()
  if (!chart && chartEl.value) chart = initChart(chartEl.value)
  if (chart) {
    chart.setOption({
      grid: { ...defaultGrid, top: 40 },
      tooltip: tooltipStyle,
      legend: { ...legendStyle, data: series.map((s) => s.name) },
      xAxis: { ...categoryAxisStyle, data: times },
      yAxis: valueAxisStyle,
      series: series.map((s) => ({
        name: s.name, type: 'line', data: s.data, smooth: true,
        lineStyle: { color: s.color, width: 2 },
        itemStyle: { color: s.color },
        areaStyle: { opacity: 0.05 },
      })),
    })
  }
}

async function loadPoints() {
  try {
    const devs = await listDevices()
    const opts: PointOption[] = []
    for (const d of devs) {
      try {
        const pts = await listPoints(d.id)
        for (const p of pts.slice(0, 10)) {
          opts.push({ id: p.id, pointName: p.name || p.id, deviceName: d.name || d.id, color: '' })
        }
      } catch { /* ignore */ }
    }
    allPoints.value = opts
  } catch { /* ignore */ }
}

let resizeHandler: (() => void) | null = null
onMounted(() => {
  loadPoints()
  resizeHandler = () => chart?.resize()
  window.addEventListener('resize', resizeHandler)
})
onUnmounted(() => {
  if (resizeHandler) {
    window.removeEventListener('resize', resizeHandler)
    resizeHandler = null
  }
  disposeChart(chart)
})
</script>

<style scoped>
.compare { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.selected-count { font-size: 0.75rem; color: var(--text-secondary); font-weight: 400; }
.time-range { display: flex; gap: 12px; align-items: center; margin-top: 12px; }
.query-hint { font-size: 0.75rem; color: var(--text-secondary); }
.compare-chart { height: 360px; }
</style>