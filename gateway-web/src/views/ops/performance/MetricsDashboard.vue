<template>
  <div class="metrics-dashboard">
    <!-- W76: 与 VISUAL_SPEC §8.2 一致——使用 el-card + section-header::before 渐变装饰 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="运行指标" icon="Histogram">
      <el-button size="small" @click="toggleAutoRefresh">
      <el-icon><Refresh /></el-icon>
      {{ autoRefresh ? '暂停刷新' : '自动刷新(5s)' }}
      </el-button>
      <!-- 必须写成 loadMetrics()：写成 loadMetrics 会把 MouseEvent 当第一个参数传入 -->
      <el-button size="small" type="primary" @click="loadMetrics()" :loading="loading">
      <el-icon><Refresh /></el-icon> 刷新
      </el-button>
    </SectionHeader>
      </template>
      <SectionHeader title="网关核心指标" icon="DataLine" class="section-header-block">
        <template #extra>
          <el-tag v-if="lastUpdate" size="small" type="info" effect="plain" round>
            {{ lastUpdate }}
          </el-tag>
        </template>
      </SectionHeader>

      <!-- 骨架分支：overview-cards + 分组列表占位 -->
      <template v-if="loading">
        <div class="overview-cards">
          <div v-for="i in 4" :key="`ov-${i}`" class="overview-card sk-card">
            <el-skeleton animated>
              <template #template>
                <el-skeleton-item variant="rect" class="sk-ov-num" />
                <el-skeleton-item variant="text" />
              </template>
            </el-skeleton>
          </div>
        </div>
        <div class="metric-groups">
          <TableSkeleton :rows="4" :columns="3" label="正在加载运行指标" />
        </div>
      </template>

      <template v-else>
        <div class="overview-cards">
          <div v-for="item in overviewItems" :key="item.label" class="overview-card">
            <div class="overview-value" :style="{ color: item.color }">{{ item.value }}</div>
            <div class="overview-label">{{ item.label }}</div>
          </div>
        </div>

        <!-- 指标分组列表 -->
        <el-collapse v-model="activeGroups" class="metric-groups">
          <el-collapse-item
            v-for="group in metricGroups"
            :key="group.name"
            :name="group.name"
          >
            <template #title>
              <div class="group-title">
                <el-tag size="small" effect="plain" round>{{ group.samples.length }}</el-tag>
                <span class="group-name">{{ group.name }}</span>
                <span class="group-help" v-if="group.help">{{ group.help }}</span>
                <el-tag size="small" type="info" effect="plain" round>{{ group.type }}</el-tag>
              </div>
            </template>
            <DataTable :data="group.samples" :columns="sampleColumns" row-key="uid" stripe empty-text="该分组暂无采样数据" />
          </el-collapse-item>
        </el-collapse>

        <empty-state
          v-if="metricGroups.length === 0"
          description="网关尚未上报指标，请确认采集任务运行中或点击刷新"
          action-text="刷新"
          :image-size="80"
          @action="loadMetrics()"
        />
      </template>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, h, type VNodeChild } from 'vue'
import { usePolling } from '@/composables/core/usePolling'
import { ElMessage, ElTag } from 'element-plus'
import {Refresh} from '@element-plus/icons-vue'
import { getMetrics } from '@/api/status'
import { parsePrometheusLines } from '@/utils/prometheus'
import type { ColumnDef } from '@/components/DataTable.vue'
import TableSkeleton from '@/components/TableSkeleton.vue'
import { formatClock, formatDateTime } from '@/utils/datetime'

const REFRESH_INTERVAL = 5000

interface MetricSample {
  uid: string
  labels: Record<string, string>
  value: number
  timestamp?: number
}

interface MetricGroup {
  name: string
  help: string
  type: string
  samples: MetricSample[]
}

// 分组样本表：标签列用 cell 平铺 el-tag（多元素需包一层容器，不能直接返回数组）
const sampleColumns: ColumnDef<MetricSample>[] = [
  {
    key: 'labels',
    title: '标签',
    minWidth: 200,
    ellipsis: true,
    cell: (row) => {
      const entries = Object.entries(row.labels)
      const children: VNodeChild[] = entries.map(([k, v]) =>
        h(ElTag, { key: k, size: 'small', type: 'info', effect: 'plain', style: 'margin-right: 4px; margin-bottom: 2px;' }, () => `${k}=${v}`),
      )
      if (!entries.length) children.push(h('span', { class: 'no-labels' }, '无'))
      return h('div', { class: 'label-list' }, children)
    },
  },
  { key: 'value', title: '数值', width: 140, cell: (row) => h('span', { class: 'metric-value' }, formatNum(row.value)) },
  { key: 'timestamp', title: '时间戳', width: 180, cell: (row) => h('span', null, row.timestamp ? formatTs(row.timestamp) : '无') },
]

const loading = ref(false)
const autoRefresh = ref(true)
const lastUpdate = ref('')
const metricGroups = ref<MetricGroup[]>([])
const activeGroups = ref<string[]>([])
let unmounted = false

const overviewItems = computed(() => {
  const findVal = (name: string) => {
    const g = metricGroups.value.find((g) => g.name === name)
    if (!g || !g.samples.length) return null
    return g.samples[0]?.value
  }
  const items: { label: string; value: string; color: string }[] = []
  const uplinkTotal = findVal('gateway_uplink_total') ?? findVal('uplink_total')
  if (uplinkTotal !== null) items.push({ label: '上行总数', value: String(uplinkTotal), color: 'var(--color-primary)' })
  const queueLen = findVal('gateway_queue_length') ?? findVal('queue_length')
  if (queueLen !== null) items.push({ label: '队列长度', value: String(queueLen), color: 'var(--text-warning)' })
  const devicesOnline = findVal('gateway_devices_online') ?? findVal('devices_online')
  if (devicesOnline !== null) items.push({ label: '在线设备', value: String(devicesOnline), color: 'var(--text-success)' })
  const errorsTotal = findVal('gateway_errors_total') ?? findVal('errors_total')
  if (errorsTotal !== null) items.push({ label: '错误总数', value: String(errorsTotal), color: 'var(--text-danger)' })
  if (!items.length) {
    const total = metricGroups.value.reduce((s, g) => s + g.samples.length, 0)
    items.push({ label: '指标总数', value: String(total), color: 'var(--color-primary)' })
  }
  return items
})

function formatNum(v: number): string {
  if (!isFinite(v)) return String(v)
  if (Number.isInteger(v)) return v.toLocaleString('zh-CN')
  return v.toFixed(2)
}

function formatTs(ts: number): string {
  try {
    return formatDateTime(ts * 1000)
  } catch {
    return String(ts)
  }
}

/**
 * 解析 Prometheus text 格式为带 HELP/TYPE 元数据的分组。
 * 行分词复用 utils/prometheus（与 api/status.ts 的扁平聚合共享同一底层）。
 */
function parsePrometheus(text: string): MetricGroup[] {
  const groups: Record<string, MetricGroup> = {}
  for (const line of parsePrometheusLines(text)) {
    if (line.kind === 'help') {
      if (!groups[line.name]) groups[line.name] = { name: line.name, help: line.text, type: '', samples: [] }
      else groups[line.name]!.help = line.text
      continue
    }
    if (line.kind === 'type') {
      if (!groups[line.name]) groups[line.name] = { name: line.name, help: '', type: line.metricType, samples: [] }
      else groups[line.name]!.type = line.metricType
      continue
    }
    const { name, labels, value, timestamp } = line.sample
    // W77: label 解析在公共分词器中已处理转义——允许 label 值中含 \" 等转义字符。
    // Prometheus 规范中 label 值不应含未转义的 "，分词器对非法字符仅跳过不报错，
    // 避免服务端异常导致整个解析失败。
    if (!groups[name]) groups[name] = { name, help: '', type: '', samples: [] }
    const uid = `${name}_${JSON.stringify(labels)}_${groups[name]!.samples.length}`
    groups[name]!.samples.push({ uid, labels, value, timestamp })
  }
  return Object.values(groups)
}

/**
 * @param opts.silent 静默刷新（定时器路径）：不置 loading，避免每 5 秒让指标面板被
 *                    蒙层覆盖、刷新按钮同步抖动。手动点「刷新」仍显示遮罩与按钮 loading。
 */
async function loadMetrics(opts: { silent?: boolean } = {}) {
  const silent = opts.silent === true
  if (!silent) loading.value = true
  try {
    const raw = await getMetrics()
    if (typeof raw === 'string') {
      metricGroups.value = parsePrometheus(raw)
            activeGroups.value = metricGroups.value.slice(0, 5).map((g) => g.name)
    }
    lastUpdate.value = formatClock()
  } catch {
    if (!unmounted) ElMessage.error('获取指标失败')
  } finally {
    if (!silent) loading.value = false
  }
}

// 自动刷新轮询：usePolling 统一提供隐藏暂停 / 失败退避 / 卸载清理。
// 定时器路径静默刷新：loadMetrics 的入参是对象，包装成无参函数再交给轮询。
const { start: startPoll, stop: stopPoll } = usePolling(
  () => loadMetrics({ silent: true }),
  REFRESH_INTERVAL,
  { autoStart: false },
)

function toggleAutoRefresh() {
  autoRefresh.value = !autoRefresh.value
  if (autoRefresh.value) {
    startPoll()
    // W79: 开启后立即加载一次，避免用户等待 5 秒才能看到首批数据。
    loadMetrics()
  } else {
    stopPoll()
  }
}

onMounted(() => {
  loadMetrics() // 首次加载：显示遮罩
  if (autoRefresh.value) startPoll()
})

onUnmounted(() => {
  unmounted = true
})
</script>

<style scoped>
.metrics-dashboard { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.section-header-block { margin-bottom: 12px; }
.metrics-body { min-height: 200px; }
.overview-cards {
  display: grid; grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
  gap: 12px; margin-bottom: 20px;
}
.overview-card {
  text-align: center; padding: 16px 8px;
  background: var(--bg-fill); border-radius: var(--radius-card);
  border: 1px solid var(--border-extralight);
  transition: border-color 0.2s;
}
.overview-card:hover { border-color: var(--color-primary); }
.overview-value { font-size: 1.5rem; font-weight: 700; font-variant-numeric: tabular-nums; }
.overview-label { font-size: 0.75rem; color: var(--text-secondary); margin-top: 4px; }
.metric-groups { margin-top: 8px; }
.group-title { display: flex; align-items: center; gap: 8px; width: 100%; }
.group-name { font-weight: 600; font-size: 0.875rem; }
.group-help { color: var(--text-secondary); font-size: 0.75rem; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.label-list { display: flex; flex-wrap: wrap; }
.no-labels { color: var(--text-placeholder); }
.metric-value { font-weight: 600; font-variant-numeric: tabular-nums; color: var(--color-primary); }

/* 骨架占位 */
.sk-card { padding: 12px; }
.sk-ov-num { height: 28px; width: 70%; margin: 0 auto 6px; border-radius: var(--radius-small); }
</style>
