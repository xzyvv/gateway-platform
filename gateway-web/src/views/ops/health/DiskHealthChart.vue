<template>
  <div class="disk-health">
    <SectionHeader title="磁盘健康" icon="Coin">
        <el-button @click="toggleAutoRefresh">
        <el-icon><Refresh /></el-icon>
        {{ autoRefresh ? '暂停自动刷新' : '自动刷新(30s)' }}
        </el-button>
        <!-- 必须写成 load()：写成 load 会把 MouseEvent 当第一个参数传入 -->
        <el-button type="primary" @click="load()" :loading="loading">
        <el-icon><Refresh /></el-icon> 刷新
        </el-button>
      </SectionHeader>

    <div>
      <!-- 首屏骨架：概要卡片 + 图表区域同构占位，数据到达时布局不跳 -->
      <template v-if="loading && !history">
        <el-card shadow="never" class="summary-card">
          <el-skeleton animated>
            <template #template>
              <div class="summary-row">
                <div class="summary-item" v-for="i in 3" :key="`sk-summary-${i}`">
                  <el-skeleton-item variant="text" class="sk-summary-value" />
                  <el-skeleton-item variant="text" class="sk-summary-label" />
                </div>
              </div>
            </template>
          </el-skeleton>
        </el-card>

        <el-card shadow="never" class="chart-card">
          <el-skeleton animated>
            <template #template>
              <el-skeleton-item variant="rect" class="sk-chart-rect" />
              <div class="sk-legend">
                <el-skeleton-item
                  variant="text"
                  v-for="i in 3"
                  :key="`sk-legend-${i}`"
                  class="sk-legend-item"
                />
              </div>
            </template>
          </el-skeleton>
        </el-card>
      </template>

      <template v-else>
        <!-- 当前剩余空间大数 -->
        <el-card v-if="history && history.samples.length > 0" shadow="never" class="summary-card">
          <div class="summary-row">
            <div class="summary-item">
              <div class="summary-value" :style="{ color: currentColor }">{{ currentFreePct }}%</div>
              <div class="summary-label">当前剩余空间</div>
            </div>
            <div class="summary-item">
              <div class="summary-value" style="color: var(--text-info)">{{ history.count }}</div>
              <div class="summary-label">采样点数</div>
            </div>
            <div class="summary-item">
              <div class="summary-value" style="color: var(--text-warning)">{{ DANGER_THRESHOLD }}%</div>
              <div class="summary-label">危险阈值</div>
            </div>
          </div>
        </el-card>

        <empty-state
          v-if="history && history.samples.length === 0"
          description="暂无磁盘采样数据"
        />

        <el-card v-if="history && history.samples.length > 0" shadow="never" class="chart-card">
          <div ref="chartRef" class="chart" data-chart-label="磁盘容量与健康趋势图"></div>
        </el-card>
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, nextTick, watch } from 'vue'
import { usePolling } from '@/composables/core/usePolling'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import {Refresh} from '@element-plus/icons-vue'
import echarts, { initChart, disposeChart } from '@/utils/echarts-extra'
import { defaultGrid, valueAxisStyle, categoryAxisStyle, axisNameStyle } from '@/utils/chartStyle'
import { CHART_PRIMARY, CHART_DANGER } from '@/utils/chartTheme'
import { getDiskHistory, type DiskHistory } from '@/api/status'
import { logError } from '@/utils/logger'

const DANGER_THRESHOLD = 10
const REFRESH_INTERVAL = 30000

const chartRef = ref<HTMLElement>()
let chart: echarts.ECharts | null = null

const history = ref<DiskHistory | null>(null)
const loading = ref(false)
const autoRefresh = ref(false)

const currentFreePct = computed(() => {
  const samples = history.value?.samples ?? []
  if (!samples.length) return 0
  return Math.round(samples[samples.length - 1]!.free_pct)
})

// 摘要数值是文字色：用 --text-* 变体（原色在白底仅 2.04~2.90:1，不达 WCAG AA）
const currentColor = computed(() => {
  return currentFreePct.value <= DANGER_THRESHOLD
    ? 'var(--text-danger)'
    : currentFreePct.value <= 30
      ? 'var(--text-warning)'
      : 'var(--text-success)'
})

function formatTime(ts: number): string {
  try {
    const d = new Date(ts)
    const pad = (n: number) => String(n).padStart(2, '0')
    return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`
  } catch {
    return String(ts)
  }
}

function render() {
  if (!chartRef.value || !history.value || !history.value.samples.length) return
  if (!chart) chart = initChart(chartRef.value)

  const samples = history.value.samples
  chart.setOption({
    tooltip: {
      trigger: 'axis',
      formatter: (params: { value: (string | number)[]; seriesName: string; axisValue?: string }[]) => {
        const p = params?.[0]
        if (!p) return ''
        return `${p.axisValue}<br/>剩余空间: <b>${p.value}%</b>`
      },
    },
    grid: defaultGrid,
    xAxis: {
      ...categoryAxisStyle,
      data: samples.map((s) => formatTime(s.ts)),
      boundaryGap: false,
    },
    yAxis: {
      ...valueAxisStyle,
      min: 0,
      max: 100,
      name: '剩余空间(%)',
      nameTextStyle: axisNameStyle,
      // 轴标签色由主题提供（gw-light/gw-dark），canvas 不解析 var()
      axisLabel: { formatter: '{value}%' },
    },
    series: [
      {
        type: 'line',
        smooth: true,
        symbol: 'circle',
        symbolSize: 4,
        data: samples.map((s) => s.free_pct),
        lineStyle: { width: 2, color: CHART_PRIMARY },
        itemStyle: { color: CHART_PRIMARY },
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: 'rgba(99, 102, 241, 0.35)' },
            { offset: 1, color: 'rgba(99, 102, 241, 0.02)' },
          ]),
        },
        markLine: {
          silent: true,
          symbol: 'none',
          lineStyle: { color: CHART_DANGER, type: 'dashed', width: 1.5 },
          label: {
            formatter: `危险阈值 ${DANGER_THRESHOLD}%`,
            color: CHART_DANGER,
            fontSize: 11,
            position: 'insideEndTop',
          },
          data: [{ yAxis: DANGER_THRESHOLD }],
        },
      },
    ],
  })
}

/**
 * @param opts.silent 自动刷新路径：不置 loading，避免每 30s 让整块磁盘健康面板
 *                    被 v-loading 蒙层覆盖。手动点「刷新」仍显示遮罩与按钮 loading。
 */
async function load(opts: { silent?: boolean } = {}) {
  const silent = opts.silent === true
  if (!silent) loading.value = true
  try {
    history.value = await getDiskHistory()
    await nextTick()
    render()
  } catch (e) {
    showToast(e, '加载磁盘健康数据失败')
    logError('disk-h', e)
  } finally {
    if (!silent) loading.value = false
  }
}

// 自动刷新轮询：usePolling 统一提供隐藏暂停 / 失败退避 / 卸载清理。
// 自动刷新走静默路径：load 的入参是对象，包装成无参函数再交给轮询。
const { start: startPoll, stop: stopPoll } = usePolling(() => load({ silent: true }), REFRESH_INTERVAL, { autoStart: false })

function toggleAutoRefresh() {
  autoRefresh.value = !autoRefresh.value
  if (autoRefresh.value) {
    startPoll()
  } else {
    stopPoll()
  }
}

function handleResize() {
  chart?.resize()
}

watch(() => history.value, () => { nextTick(render) })

onMounted(() => {
  load()
  window.addEventListener('resize', handleResize)
})

onUnmounted(() => {
  window.removeEventListener('resize', handleResize)
  disposeChart(chart)
  chart = null
})
</script>

<style scoped>
.disk-health { padding: 0px; }
.summary-card { border-radius: var(--radius-card); margin-bottom: 16px; border: 1px solid var(--border-base); }
.summary-row { display: flex; gap: 16px; }
.summary-item { flex: 1; text-align: center; padding: 8px; }
.summary-value { font-size: 2rem; font-weight: 700; line-height: 1.3; font-variant-numeric: tabular-nums; }
.summary-label { font-size: 0.8125rem; color: var(--text-secondary); margin-top: 4px; }
.chart-card { border-radius: var(--radius-card); border: 1px solid var(--border-base); }
.chart { height: 360px; width: 100%; }

/* 首屏骨架占位：概要卡片与图表区域按真实尺寸占位 */
.sk-summary-value { width: 60%; height: 32px; margin: 0 auto 8px; }
.sk-summary-label { width: 50%; height: 13px; margin: 0 auto; }
.sk-chart-rect { height: 336px; border-radius: var(--radius-table); }
.sk-legend { display: flex; justify-content: center; gap: 16px; margin-top: 12px; }
.sk-legend-item { width: 64px; height: 12px; }
</style>
