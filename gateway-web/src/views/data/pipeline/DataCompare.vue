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
        :disabled="loading"
      >
        <el-option v-for="p in allPoints" :key="p.id" :label="`${p.deviceName} / ${p.pointName}`" :value="p.id" />
      </el-select>
      <div class="time-range">
        <el-radio-group v-model="timeRange" size="small" :disabled="loading">
          <el-radio-button value="1">最近 1 小时</el-radio-button>
          <el-radio-button value="6">最近 6 小时</el-radio-button>
          <el-radio-button value="24">最近 24 小时</el-radio-button>
          <el-radio-button value="168">最近 7 天</el-radio-button>
        </el-radio-group>
        <el-button type="primary" size="small" :disabled="selectedPointIds.length === 0" :loading="loading" @click="loadData">
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
      <!-- DD-53：/api/history 默认只返回窗口内最新 1000 条；忽略 truncated 会让用户
           把「部分数据」当成「全部数据」下结论，这里必须显式提示。 -->
      <el-alert
        v-if="truncation"
        type="warning"
        show-icon
        :closable="false"
        class="trunc-alert"
        :title="`数据已截断：${truncation.point} 窗口内共 ${truncation.total.toLocaleString()} 条，仅显示最近 ${truncation.returned.toLocaleString()} 条`"
      >
        <span>趋势图与统计摘要只基于已返回的数据；如需完整数据请缩短时间范围或改用降采样查询。</span>
      </el-alert>
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
import {ref, computed, onMounted, onUnmounted, nextTick, h, watch} from 'vue'
import {Search} from '@element-plus/icons-vue'
import echarts, { disposeChart, ensureChart } from '@/utils/chart/echarts'
import { tooltipStyle, defaultGrid, valueAxisStyle, categoryAxisStyle, legendStyle } from '@/utils/chart/chartStyle'
import { fetchHistorySamples } from '@/composables/data/useHistoryQuery'
import { useRequestGuard } from '@/composables/core/useRequestGuard'
import { listDevices } from '@/api/acquisition/devices'
import { listPoints } from '@/api/acquisition/points'
import type { ColumnDef } from '@/components/DataTable.vue'
import { formatTimeOfDay } from '@/utils/time/datetime'
import { CHART_PRIMARY, CHART_SUCCESS, CHART_WARNING, CHART_DANGER } from '@/utils/chart/chartTheme'

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
/** DD-53：窗口内数据被 /api/history 截断时的提示（取截断最严重的点位） */
const truncation = ref<{ point: string; total: number; returned: number } | null>(null)
// 查询进行中：禁用选择器与查询按钮，避免 await 期间改选择导致结果与选择错位
const loading = ref(false)
// 查询序号守卫：旧查询的结果不得覆盖新查询
const queryGuard = useRequestGuard()

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

  // 快照本次查询的点位：await 期间用户仍可改选择（el-select 未禁用），
  // 若继续读 computed 的 selectedPoints，长度会变短，
  // 后面的 selectedPoints.value[j]! 非空断言就会越界抛 TypeError。
  const picked = [...selectedPoints.value]
  // 竞态守卫：连续点两次查询（或查询后改时间范围再查）时，
  // 先发的请求后到会用旧数据覆盖新结果。
  const seq = queryGuard.next()
  loading.value = true

  const series: { name: string; data: number[]; color: string }[] = []
  const rows: Record<string, string | number>[] = []
  const statsList: typeof stats.value = []

  // Fetch real history data for each selected point
  const results = await Promise.all(
    picked.map(async (p) => {
      try {
        // P2-1：返回体为信封内的 { items, total, ... } 而非裸数组
        // W1：不再视图内手拼 query 串，统一走 `/api/history` 的唯一取数入口
        const res = await fetchHistorySamples({ pointId: p.id, start, end: now })
        return {
          point: p,
          data: res?.items ?? [],
          // DD-53：保留截断元信息，交给下方统一提示
          total: res?.total ?? 0,
          truncated: Boolean(res?.truncated || res?.has_more),
        }
      } catch {
        return {
          point: p,
          data: [] as { timestamp: number; value: number }[],
          total: 0,
          truncated: false,
        }
      }
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
      // 用快照而非实时 computed：results 与 picked 一一对应，长度始终一致
      const sp = picked[j]!
      const sd = series[j]!.data
      row[sp.id] = sd[i] ?? '-'
    }
    rows.push(row)
  }

  // 旧请求的结果不得写入（已有更新的查询发出）。
  // 注意这里不复位 loading：当前那一轮才负责把它置回 false。
  if (!queryGuard.isCurrent(seq)) return

  // DD-53：截断信息与数据一起提交（同样受竞态守卫保护）
  const worst = results
    .filter((r) => r.truncated)
    .sort((a, b) => b.total - a.total)[0]
  truncation.value = worst
    ? {
        point: `${worst.point.deviceName}/${worst.point.pointName}`,
        total: worst.total,
        returned: worst.data.length,
      }
    : null

  dataRows.value = rows
  stats.value = statsList

  await nextTick()
  // 宿主被 v-if 卸载后重新创建时，旧实例绑定的是已移除的 DOM。
  // ensureChart 比对 getDom()，宿主变了就重建，否则 setOption 会写向
  // 已销毁的 DOM —— 新宿主的图表区将永久空白。
  chart = ensureChart(chart, chartEl.value)
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
  loading.value = false
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
  chart = null
})

// 图表宿主被 v-if 卸载（清空所选点位）时，实例绑定的 DOM 已被移除。
// 此时释放实例，避免它留在 liveCharts 注册表里泄漏；
// 条件恢复时 ensureChart 会按新宿主重建。
watch(chartEl, (el) => {
  if (!el && chart) {
    disposeChart(chart)
    chart = null
  }
})
</script>

<style scoped>
.compare { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.selected-count { font-size: 0.75rem; color: var(--text-secondary); font-weight: 400; }
.time-range { display: flex; gap: 12px; align-items: center; margin-top: 12px; }
.query-hint { font-size: 0.75rem; color: var(--text-secondary); }
.compare-chart { height: 360px; }
.trunc-alert { margin-top: 8px; }
</style>