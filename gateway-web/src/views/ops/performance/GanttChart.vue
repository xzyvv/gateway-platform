<template>
  <div class="gt">
    <el-row :gutter="16">
      <el-col :span="16">
        <el-card shadow="never" class="section-card">
          <template #header><strong>各采集任务真实执行窗口</strong>（最近 {{ WINDOW_MS / 1000 }} 秒）</template>
          <div v-if="!summary.running && summary.totalTasks === 0" class="empty-tip">
            采集调度未运行或无样本：甘特图只展示 scheduler 真实记录的执行窗口，不做推算。
          </div>
          <div ref="chartRef" data-chart-label="任务执行时序甘特图" style="height: 380px"></div>
        </el-card>
      </el-col>
      <el-col :span="8">
        <el-card shadow="never" class="section-card">
          <template #header><strong>时间窗统计</strong></template>
          <div class="stat"><span>采集组数</span><b>{{ summary.totalTasks }}</b></div>
          <div class="stat"><span>真实时间重叠</span><b :class="{ warn: summary.overlapCount > 0 }">{{ summary.overlapCount }} 处</b></div>
          <div class="bus-load" v-for="(v, k) in summary.busLoad" :key="k">
            <span>{{ k }}</span>
            <el-progress :percentage="v" :stroke-width="8" :status="v > 70 ? 'exception' : v > 40 ? 'warning' : 'success'" />
          </div>
          <el-divider />
          <div class="suggestions">
            <div v-for="(s, i) in summary.suggestions" :key="i" class="suggestion">· {{ s }}</div>
          </div>
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
/** 任务执行时序甘特图（F-11 真绑定）。
 *
 * 原实现用「配置采集间隔 + 统计平均延迟」前端推算任务条，并自行给出
 * 「检测到 N 处时间重叠」的结论——视觉精确、结论却是编造的。
 * 现在改为读取后端 `/api/diagnostics/schedule-timeline`：数据来自 scheduler
 * 采集循环尾部记录的真实执行窗口（起止 epoch 毫秒），重叠也由后端按真实样本计算。
 * 无样本时明确提示「不做推算」，不再给出虚构结论。 */
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import echarts, { initChart, disposeChart } from '@/utils/echarts-extra'
import { getScheduleTimeline } from '@/api/diagnostics'

interface TaskBar {
  label: string
  bus: string
  group: string
  blocks: { start: number; end: number; status: 'ok' | 'overrun' | 'overlap' }[]
}

const chartRef = ref<HTMLElement>()
const summary = ref({
  running: false,
  totalTasks: 0,
  overlapCount: 0,
  busLoad: {} as Record<string, number>,
  suggestions: [] as string[],
})
let chart: echarts.ECharts | null = null

const WINDOW_MS = 10000

onMounted(async () => {
  let tasks: TaskBar[] = []
  let overlaps: { a_group: string; b_group: string; start_ms: number; end_ms: number }[] = []
  let running = false

  try {
    const tl = await getScheduleTimeline(WINDOW_MS)
    running = tl.running
    // 真实窗口起点：以快照生成时刻为准，避免前后端时钟漂移
    const windowStart = tl.generated_at_ms - tl.window_ms
    overlaps = tl.overlaps
    tasks = tl.groups.slice(0, 8).map((g) => {
      const blocks: TaskBar['blocks'] = g.samples.map((s) => {
        const start = Math.max(0, s.start_ms - windowStart)
        const end = Math.min(WINDOW_MS, s.end_ms - windowStart)
        // 真实重叠标记：该样本与同总线不同组的重叠区间相交
        const hitOverlap = overlaps.some(
          (o) =>
            (o.a_group === g.group || o.b_group === g.group) &&
            s.start_ms < o.end_ms &&
            s.end_ms > o.start_ms,
        )
        return { start, end: Math.max(start, end), status: hitOverlap ? 'overlap' : s.overrun ? 'overrun' : 'ok' }
      })
      return { label: `${g.protocol}@${g.bus}`, bus: g.bus, group: g.group, blocks }
    })
  } catch {
    // 拉取失败沿用空样本：页面明确提示「不做推算」，不猜测数据
  }

  // 真实总线占用率：窗口内该总线所有组执行时长之和 ÷ 窗口长度
  const busBusy: Record<string, number> = {}
  for (const t of tasks) {
    const total = t.blocks.reduce((a, b) => a + (b.end - b.start), 0)
    busBusy[t.bus] = (busBusy[t.bus] ?? 0) + total
  }
  const busLoad: Record<string, number> = {}
  for (const b of Object.keys(busBusy)) {
    busLoad[b] = Math.min(100, Math.round(((busBusy[b] ?? 0) / WINDOW_MS) * 100))
  }

  const suggestions: string[] = []
  if (!running) suggestions.push('采集调度未运行，无真实执行窗口可展示')
  if (overlaps.length > 0) suggestions.push(`检测到 ${overlaps.length} 处真实时间重叠：同总线上不同采集组执行窗口相交`)
  for (const [b, v] of Object.entries(busLoad)) {
    if (v > 70) suggestions.push(`${b} 真实占用 ${v}%，建议减少该总线上的设备或延长采集周期`)
  }
  if (suggestions.length === 0) {
    suggestions.push(tasks.length > 0 ? '当前调度无真实时间重叠' : '暂无采集样本（采集刚启动或该窗口内无点位调度）')
  }

  summary.value = { running, totalTasks: tasks.length, overlapCount: overlaps.length, busLoad, suggestions }

  await nextTick()
  if (!chartRef.value) return
  chart = initChart(chartRef.value)
  const data: unknown[] = []
  tasks.forEach((t, di) => {
    t.blocks.forEach((b) => {
      data.push([di, b.start, b.end, b.status, t.label])
    })
  })
  chart.setOption({
    tooltip: {
      formatter: (p: { value: (string | number)[] }) =>
        `${p.value[4]}<br/>${p.value[1]}ms ~ ${p.value[2]}ms<br/>状态: ${
          p.value[3] === 'ok' ? '正常' : p.value[3] === 'overrun' ? '耗时超周期' : '同总线重叠'
        }`,
    },
    grid: { left: 120, right: 20, top: 20, bottom: 40 },
    xAxis: { type: 'value', min: 0, max: WINDOW_MS, name: '时间(ms)' },
    yAxis: { type: 'category', data: tasks.map((t) => t.label), inverse: true },
    series: [
      {
        type: 'custom',
        renderItem: (
          _params: unknown,
          api: {
            value: (i: number) => number | string
            coord: (v: number[]) => number[]
            size: (v: number[]) => number[]
          },
        ) => {
          const yIdx = Number(api.value(0))
          const start = api.coord([Number(api.value(1)), yIdx])
          const end = api.coord([Number(api.value(2)), yIdx])
          const height = api.size([0, 1])[1]! * 0.5
          const status = api.value(3)
          const fill =
            status === 'ok'
              ? 'var(--el-color-success)'
              : status === 'overrun'
                ? 'var(--el-color-warning)'
                : 'var(--el-color-danger)'
          return {
            type: 'rect',
            shape: { x: start[0]!, y: start[1]! - height / 2, width: Math.max(1, end[0]! - start[0]!), height },
            style: { fill, opacity: 0.8 },
          }
        },
        encode: { x: [1, 2], y: 0 },
        data,
      },
    ],
  })
})
// 与其余图表页保持一致：卸载时 dispose 并摘除全局注册表，
// 否则反复进出该页会让已销毁实例在 liveCharts 中单调累积。
onUnmounted(() => {
  disposeChart(chart)
  chart = null
})
</script>

<style scoped>
.gt { padding: 0px; }
.section-card { margin-bottom: 16px; }
.stat { display: flex; justify-content: space-between; padding: 8px 0; border-bottom: 1px solid var(--el-border-color-lighter); }
.stat b { font-size: 1.125rem; }
.stat .warn { color: var(--text-warning); }
.bus-load { margin: 12px 0; }
.bus-load span { font-size: 0.8125rem; color: var(--el-text-color-secondary); }
.suggestion { font-size: 0.8125rem; color: var(--el-color-primary); margin-bottom: 6px; }
.empty-tip { padding: 4px 0 8px; font-size: 0.78rem; color: var(--text-secondary); }
</style>
