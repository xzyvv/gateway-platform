<template>
  <div class="topo">
    <SectionHeader title="设备拓扑" icon="Connection" />

    <el-row :gutter="16">
      <el-col :span="5">
        <el-card shadow="never" class="section-card">
          <template #header><strong>统计</strong></template>
          <div class="stat-grid">
            <div class="stat"><span>总线</span><b>{{ stats.bus_count }}</b></div>
            <div class="stat"><span>设备</span><b>{{ stats.device_count }}</b></div>
            <div class="stat"><span class="online">在线</span><b>{{ stats.online_count }}</b></div>
            <div class="stat"><span class="offline">离线</span><b>{{ stats.offline_count }}</b></div>
            <div class="stat"><span class="warn">告警</span><b>{{ stats.warning_count }}</b></div>
            <div class="stat"><span>点位</span><b>{{ stats.point_count }}</b></div>
          </div>
        </el-card>
        <el-card shadow="never" class="section-card">
          <template #header><strong>图例</strong></template>
          <div class="legend"><span class="dot bus"></span>总线</div>
          <div class="legend"><span class="dot online"></span>在线设备</div>
          <div class="legend"><span class="dot warn"></span>告警设备</div>
          <div class="legend"><span class="dot offline"></span>离线设备</div>
          <div class="legend"><span class="dot point"></span>点位</div>
        </el-card>
      </el-col>
      <el-col :span="19">
        <el-card shadow="never" class="section-card">
          <template #header><strong>搜索 总线 / 设备 / 点位 名称</strong></template>
          <div v-if="loading" class="topo-error">正在加载拓扑数据…</div>
          <div v-else-if="loadError" class="topo-error">拓扑数据加载失败：{{ loadError }}</div>
          <div v-else-if="!hasNodes" class="topo-error">暂无拓扑数据，需先在设备基础 › 设备清单中添加设备</div>
          <!-- 必须用 v-if 而非 v-show：v-show 只切 display，画布元素仍在 DOM 中且宽高为 0，
               echarts 初始化会抛 "Can't get DOM width or height"，导致整页被错误边界接管 -->
          <div v-if="!loading && !loadError && hasNodes" ref="chartRef" data-chart-label="设备与网关拓扑关系图" class="topo-canvas"></div>
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import type echarts from '@/utils/echarts-extra'
import { initChart, disposeChart, cssVar } from '@/utils/echarts-extra'
import { CHART_PRIMARY, CHART_SUCCESS, CHART_WARNING, CHART_DANGER, CHART_MUTED_TEXT, CHART_LINE } from '@/utils/chartTheme'
import { getTopology } from '@/api/topology'

const chartRef = ref<HTMLElement>()
let chart: echarts.ECharts | null = null
const stats = ref({ bus_count: 0, device_count: 0, online_count: 0, offline_count: 0, warning_count: 0, point_count: 0 })
/** 拓扑数据加载失败原因（非空时显示错误空态，避免图表区静默空白） */
const loadError = ref('')
/** 是否已取到节点：全空时给出「暂无拓扑数据」提示而非渲染空画布 */
const hasNodes = ref(false)
/** 加载中：避免数据返回前先闪一次「暂无拓扑数据」的假空态 */
const loading = ref(true)

onMounted(async () => {
  let topo
  try {
    topo = await getTopology()
  } catch (e) {
    loadError.value = e instanceof Error ? e.message : String(e)
    loading.value = false
    return
  }
  hasNodes.value = topo.nodes.length > 0
  loading.value = false
  // 第 1 期冗余收口：`/api/devices/topology/stats` 已删除，统计随拓扑同响应返回
  if (topo.stats) stats.value = topo.stats
  await nextTick()
  if (!chartRef.value) return
  const instance = initChart(chartRef.value)
  chart = instance
  instance.setOption({
    tooltip: {
      formatter: (p: { data: { name: string; category: string; meta?: string } }) => {
        const d = p.data
        return `${d.name}<br/>类别: ${d.category}<br/>${d.meta || ''}`
      },
    },
    series: [{
      type: 'graph',
      layout: 'force',
      roam: true,
      draggable: true,
      force: { repulsion: 300, edgeLength: [80, 150] },
      categories: [
        { name: 'bus', itemStyle: { color: CHART_PRIMARY } },
        { name: 'online', itemStyle: { color: CHART_SUCCESS } },
        { name: 'warning', itemStyle: { color: CHART_WARNING } },
        { name: 'offline', itemStyle: { color: CHART_DANGER } },
        { name: 'point', itemStyle: { color: CHART_MUTED_TEXT } },
      ],
      data: topo.nodes.map((n) => ({
        id: n.id, name: n.name,
        category: n.node_type === 'bus' ? 'bus' : n.node_type === 'point' ? 'point' : (n.status || 'online'),
        meta: n.meta,
        symbolSize: n.node_type === 'bus' ? 60 : n.node_type === 'device' ? 45 : 25,
        label: { show: true, position: 'bottom', fontSize: 11 },
      })),
      links: topo.links.map((l) => ({ source: l.source, target: l.target, lineStyle: { color: cssVar('--text-placeholder', CHART_LINE), width: 1.5 } })),
      lineStyle: { curveness: 0.1 },
    }],
  })
})
// 拓扑图是排查高频页，必须在卸载时 dispose 并从全局注册表摘除，
// 否则每次进出都会往 liveCharts 永久追加一个已销毁实例（canvas + zrender）。
onUnmounted(() => {
  disposeChart(chart)
  chart = null
})
</script>

<style scoped>
.topo { padding: 16px; }
.section-card { margin-bottom: 16px; }
/* 加载失败 / 无数据：给出文字说明，避免图表区静默空白 */
.topo-error { padding: 60px 0; text-align: center; color: var(--text-secondary); font-size: 0.875rem; }
/* 统计改 2 列网格：窄侧栏内 6 项纵排过长，网格压缩高度给图例留位 */
.stat-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 0 12px; }
.stat { display: flex; justify-content: space-between; padding: 8px 0; border-bottom: 1px solid var(--el-border-color-lighter); }
.stat b { font-size: 1.125rem; }
.stat .online { color: var(--text-success); }
.stat .offline { color: var(--text-danger); }
.stat .warn { color: var(--text-warning); }
/* 画布跟随视口高度：最小 520px，避免小屏过矮、大屏浪费 */
.topo-canvas { height: max(520px, calc(100vh - 220px)); }
.legend { display: flex; align-items: center; gap: 8px; padding: 4px 0; font-size: 0.8125rem; }
.dot { width: 14px; height: 14px; border-radius: 50%; display: inline-block; }
.dot.bus { background: var(--color-primary); }
.dot.online { background: var(--color-success); }
.dot.warn { background: var(--color-warning); }
.dot.offline { background: var(--color-danger); }
.dot.point { background: var(--color-info); }
</style>
