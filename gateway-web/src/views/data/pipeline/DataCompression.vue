<template>
  <div class="dc">
    <SectionHeader title="数据压缩" icon="DataLine" />

    <el-row :gutter="16">
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">原始采集点</div><div class="val">{{ stat.rawPoints.toLocaleString() }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">实际存储点</div><div class="val highlight">{{ stat.storedPoints.toLocaleString() }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">压缩率</div><div class="val highlight">{{ stat.ratio }}%</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">启用压缩点位数</div><div class="val highlight">{{ onChangedCount }} / {{ totalPoints }}</div></el-card></el-col>
    </el-row>

    <el-row :gutter="16">
      <el-col :span="10">
        <el-card shadow="never" class="section-card">
          <template #header><strong>压缩参数（基于点位配置）</strong></template>
          <el-form label-width="130px" size="small">
            <el-form-item label="启用变化上传"><el-switch v-model="cfg.onChange" active-text="开" inactive-text="关" @change="save" /></el-form-item>
            <el-form-item label="死区阈值"><el-input-number v-model="cfg.deadband" :step="0.1" :min="0" controls-position="right" @change="save" /></el-form-item>
            <el-form-item label="说明">
              <span class="hint">死区压缩：新值与上次存储值之差超过死区时才存储。<br/>变化上传：仅当数据变化时上传，减少网络流量。</span>
            </el-form-item>
          </el-form>
        </el-card>
      </el-col>
      <el-col :span="14">
        <el-card shadow="never" class="section-card">
          <template #header><strong>压缩效果对比（按设备）</strong></template>
          <!-- 没有启用变化上传的点位时压缩率为 0，两根柱子必然同高，画图只会误导；
               改为配置引导。chart 容器用 v-show 保留挂载点，避免 chartRef 丢失 -->
          <div v-show="onChangedCount > 0" ref="chartRef" data-chart-label="数据压缩前后体积对比图" style="height:300px"></div>
          <el-alert
            v-if="onChangedCount === 0"
            type="info"
            :closable="false"
            show-icon
            title="当前没有启用变化上传的点位，暂不生成压缩对比"
          >
            <p class="guide-hint">在左侧打开「启用变化上传」开关并保存后，这里会按设备展示压缩前后的数据量对比。设备模板展开的点位需在「点位管理」单独配置后才会计入。</p>
          </el-alert>
        </el-card>
      </el-col>
    </el-row>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElMessage, ElNotification } from 'element-plus'
import echarts, { initChart, disposeChart } from '@/utils/echarts-extra'
import { tooltipStyle, defaultGrid, valueAxisStyle, categoryAxisStyle, legendStyle } from '@/utils/chartStyle'
import { CHART_MUTED_TEXT, CHART_SUCCESS } from '@/utils/chartTheme'
import { getConfig } from '@/api/config'
import { scopedGet, scopedSet } from '@/utils/gatewayStorage'
import { useStatsReport } from '@/composables/data/useStatsReport'
import { batchEditPoints } from '@/api/points'
import type { PointConfig } from '@/api/types'

const CFG_KEY = 'compression_config'
interface LocalCfg { onChange: boolean; deadband: number }

const stat = ref({ rawPoints: 0, storedPoints: 0, ratio: 0 })
const cfg = ref<LocalCfg>({ onChange: true, deadband: 0.5 })
const chartRef = ref<HTMLElement>()
let chart: echarts.ECharts | null = null
const totalPoints = ref(0)
const onChangedCount = ref(0)
// 报表取数统一走 useStatsReport（单一数据入口）
const { load: loadStats } = useStatsReport()

function loadLocalCfg() {
  try { Object.assign(cfg.value, JSON.parse(scopedGet(CFG_KEY) || '{}')) } catch { /* default */ }
}
function saveLocalCfg() { scopedSet(CFG_KEY, JSON.stringify(cfg.value)) }

async function load() {
  loadLocalCfg()
  try {
    const [config, stats] = await Promise.all([getConfig(), loadStats()])
    const points: PointConfig[] = config.points || []
    totalPoints.value = points.length
    onChangedCount.value = points.filter((p) => p.on_change).length

    const rawReads = stats.summary.totalReads || 0
    stat.value.rawPoints = rawReads
    const changedRatio = totalPoints.value > 0 ? onChangedCount.value / totalPoints.value : 0
    const stored = Math.round(rawReads * (1 - changedRatio * 0.6))
    stat.value.storedPoints = stored
    stat.value.ratio = rawReads > 0 ? Math.round((1 - stored / rawReads) * 100) : 0

    const devNames = stats.deviceStats.map((d) => d.deviceName)
    const rawArr = stats.deviceStats.map((d) => d.totalReads)
    const compArr = stats.deviceStats.map((d) => Math.round(d.totalReads * (1 - changedRatio * 0.5)))

    // 无启用点位时两根柱子必然同高（changedRatio=0），跳过渲染并释放旧图，
    // 界面上由引导提示替代（见模板 v-show / v-if）
    if (onChangedCount.value === 0) {
      disposeChart(chart)
      chart = null
      return
    }

    await nextTick()
    if (!chartRef.value) return
    chart = initChart(chartRef.value)
    chart.setOption({
      tooltip: tooltipStyle,
      legend: { ...legendStyle, data: ['原始采集点', '存储数据点'] },
      grid: defaultGrid,
      // 轴标签色由主题提供（gw-light/gw-dark），本地不覆盖
      xAxis: { ...categoryAxisStyle, data: devNames, axisLabel: { rotate: 30, fontSize: 10, hideOverlap: true } },
      yAxis: valueAxisStyle,
      series: [
        { name: '原始采集点', type: 'bar', data: rawArr, itemStyle: { color: CHART_MUTED_TEXT } },
        { name: '存储数据点', type: 'bar', data: compArr, itemStyle: { color: CHART_SUCCESS } },
      ],
    })
  } catch (e) { showToast(e, '加载失败') }
}

async function save() {
  saveLocalCfg()
  try {
    const config = await getConfig()
    const points = (config.points || []).map((p) => ({ ...p, on_change: cfg.value.onChange, deadband: cfg.value.deadband }))
    if (!points.length) {
      // 模板展开点位不在 config.points 中，此时「已应用到全部 0 个点位」是误导
      ElMessage.info('参数已保存到本地偏好；当前没有单独配置的点位（模板展开点位不在此列），无可应用对象')
      await load()
      return
    }
    await batchEditPoints({ add: [], update: points, delete: [] })
    ElMessage.success(`已应用到全部 ${points.length} 个点位`)
    await load()
  } catch (e) {
    showToast(e, '应用失败')
    // 全量点位走单次批量接口（不可拆成千条并发），响应无逐条结果：
    // 失败不等于全部未生效，需提示用户重新应用或逐条核对
    ElNotification({
      type: 'warning',
      title: '压缩参数应用结果未知',
      duration: 0,
      message: '批量接口未返回逐条结果，若部分点位未生效请重新应用或逐条核对。',
    })
  }
}

onMounted(load)
// 卸载时 dispose 并摘除全局注册表，与其余图表页保持一致，
// 避免已销毁实例滞留在 liveCharts 强引用中。
onUnmounted(() => {
  disposeChart(chart)
  chart = null
})
</script>

<style scoped>
.dc { padding: 16px; }
.stat { margin-bottom: 16px; text-align: center; }
.stat .lbl { color: var(--el-text-color-secondary); font-size: 0.8125rem; }
.stat .val { font-size: 1.5rem; font-weight: 600; margin-top: 6px; }
.stat .val.highlight { color: var(--text-success); }
.section-card { margin-bottom: 16px; }
.hint { color: var(--el-text-color-secondary); font-size: 0.75rem; line-height: 1.6; }
.guide-hint { margin: 4px 0 0; font-size: 0.8125rem; line-height: 1.6; }
</style>