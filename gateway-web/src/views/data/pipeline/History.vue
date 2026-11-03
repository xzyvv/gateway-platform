<template>
  <div class="history">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="查询条件" icon="Search" />
      </template>

      <el-form :model="query" class="query-form">
        <div class="query-row">
          <el-form-item label="点位 ID" class="query-item point-item">
            <el-autocomplete
              v-model="query.point_id"
              :fetch-suggestions="suggestPoints"
              :trigger-on-focus="true"
              placeholder="单个或多个（逗号分隔）如: meter1_voltage, meter1_current"
              clearable
              style="width: 100%"
            />
          </el-form-item>
          <el-form-item label="降采样(秒)" class="query-item downsample-item">
            <el-input-number v-model="query.downsample" :min="0" :max="1000" controls-position="right" />
          </el-form-item>
          <el-form-item label="聚合粒度" class="query-item granularity-item">
            <el-select v-model="granularity" style="width: 120px">
              <el-option label="原始" value="raw" />
              <el-option label="按小时" value="hour" />
              <el-option label="按天" value="day" />
            </el-select>
          </el-form-item>
          <el-form-item class="query-item btn-item">
            <el-button type="primary" @click="runQuery">
              <el-icon><Search /></el-icon> 查询
            </el-button>
            <el-select v-model="exportFormat" size="small" style="width: 88px" aria-label="导出格式">
              <el-option label="CSV" value="csv" />
              <el-option label="JSON" value="json" />
            </el-select>
            <el-button @click="doExport" :disabled="!history.length">
              <el-icon><Download /></el-icon> 导出
            </el-button>
          </el-form-item>
        </div>

        <div class="query-row">
          <el-form-item label="开始时间" class="query-item time-item">
            <el-date-picker v-model="query.start" type="datetime" value-format="x" style="width: 100%" />
          </el-form-item>
          <el-form-item label="结束时间" class="query-item time-item">
            <el-date-picker v-model="query.end" type="datetime" value-format="x" style="width: 100%" />
          </el-form-item>
          <!-- 快捷选择并到时间行，少一层纵向堆叠，首屏能多露出一截图表 -->
          <div class="quick-time">
            <span class="quick-label">快捷选择：</span>
            <el-button-group>
              <el-button
                v-for="r in quickRanges"
                :key="r.key"
                size="small"
                :type="activeQuickKey === r.key ? 'primary' : 'default'"
                @click="setQuickRange(r.key)"
              >{{ r.label }}</el-button>
            </el-button-group>
          </div>
        </div>
      </el-form>
    </el-card>

    <!-- 结果卡常驻：首次进页面不再是一张空表单，下面的空态直接告诉用户下一步点哪里 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="查询结果" icon="TrendCharts">
          <template #extra>
            <el-tag v-if="history.length" type="info" effect="plain" round size="small" class="count-tag">
              共 {{ displayHistory.length }} 条{{ granularity !== 'raw' ? '（聚合）' : '' }}
            </el-tag>
            <el-tag v-else-if="loading" type="warning" effect="plain" round size="small" class="count-tag">
              查询中...
            </el-tag>
            <el-tag v-else type="info" effect="plain" round size="small" class="count-tag">
              {{ queried ? '暂无数据' : '待查询' }}
            </el-tag>
          </template>
        </SectionHeader>
      </template>

      <!--
        P2-1③：后端单次查询只保留窗口内最新 limit 条。截断时必须显式告知，
        否则用户会把"数据变少"误判为采集丢失。
      -->
      <el-alert
        v-if="truncation"
        class="truncation-alert"
        type="warning"
        :closable="false"
        show-icon
        :title="`数据已截断：窗口内共 ${truncation.total} 条，仅显示最近 ${truncation.returned} 条`"
      >
        点位 {{ truncation.pointId }} 的数据超出单次返回上限。可缩小时间范围，
        或调大「降采样(秒)」让后端按时间桶聚合（桶内给出均值/最小值/最大值）。
      </el-alert>

      <div class="stats-row" v-if="displayHistory.length">
        <StatCard label="数据点数" :value="history.length" />
        <StatCard label="最小值" :value="stats.min" color="var(--text-success)" />
        <StatCard label="最大值" :value="stats.max" color="var(--text-danger)" />
        <StatCard label="平均值" :value="stats.avg" color="var(--color-primary)" />
      </div>

      <div class="chart-wrap">
        <div :ref="setChartEl" class="chart" data-chart-label="历史数据趋势图"></div>
        <div v-if="multiHistory.length === 0 && !loading" class="chart-empty">
          <el-icon :size="40"><Histogram /></el-icon>
          <p>{{ emptyHint }}</p>
          <el-button v-if="queried" size="small" @click="widenTo24h">放宽到近 24 小时</el-button>
        </div>
      </div>

      <el-tabs
        v-if="multiHistory.length > 1"
        v-model="activePointId"
        class="point-tabs"
        type="card"
      >
        <el-tab-pane
          v-for="m in multiHistory"
          :key="m.pointId"
          :name="m.pointId"
          :label="`${m.pointId} (${m.data.length})`"
        />
      </el-tabs>

      <!--
        A4: 表格统一走 DataTable，长查询结果（万级/十万级行）自动切虚拟滚动。
        无数据且非加载中时表格不渲染：图表区的空态已经给了引导文案，
        再叠一个空的表头 + 同样的空态就是重复提示。
      -->
      <DataTable
        v-if="displayHistory.length || loading"
        :loading="loading"
        :data="displayHistory"
        :columns="historyColumns"
        :row-key="historyRowKey"
        :estimated-row-height="36"
      >
        <template #empty>
          <empty-state :description="emptyHint" :image-size="80" />
        </template>
      </DataTable>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h, computed, watch, ref, nextTick, onMounted } from 'vue'
import { ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import StatCard from '@/components/StatCard.vue'
import { useHistoryQuery } from '@/composables/data/useHistoryQuery'
import { useQueryState } from '@/composables/core/useQueryState'
import { fetchPointBackoffOnce } from '@/composables/data/usePointBackoff'
import { fetchPointQualities } from '@/composables/data/usePointQuality'
import { usePointOptions } from '@/composables/data/usePointOptions'
import { qualityText } from '@/utils/qualityLabels'
import { scopedGet, scopedSet } from '@/utils/gatewayStorage'

/** 历史行：原始查询与聚合结果共用同一形状 */
type HistoryRow = { timestamp: number; value: number; quality: string; rowKey?: string }

/** DataTable 行键：优先用后端行键，缺失时以「时间戳_值」兜底 */
const historyRowKey = (row: HistoryRow) => row.rowKey ?? `${row.timestamp}_${row.value}`

/** 点位候选：从采集退避接口拿当前活跃点位（含模板展开点位），聚焦/输入时提示。
 *  此前要求手输完整点位 ID（如 sim-meter_voltage），填「sim-meter」或数字 ID
 *  查不到数据，是历史查询「永远为空」的最常见原因。 */
const pointCandidates = ref<{ value: string }[]>([])
/** 点位选项（配置点位 + 运行时快照合并）：候选来源之一，保证无采集器时也能选到点位 */
const { pointOptions, reload: reloadPoints } = usePointOptions()
function suggestPoints(qs: string, cb: (items: { value: string }[]) => void) {
  const kw = qs.trim().toLowerCase()
  const pool = kw
    ? pointCandidates.value.filter((c) => c.value.toLowerCase().includes(kw))
    : pointCandidates.value
  cb(pool.slice(0, 20))
}

const {
  query, quickRanges, activeQuickKey,
  history, granularity, activePointId, multiHistory,
  truncation,
  displayHistory, stats,
  loading, queried,
  qualityTagType,
  setQuickRange, queryHistory, exportCsv, exportJson, formatTime, renderChart, chartRef,
} = useHistoryQuery()

/** 导出格式选择（原「导出格式」页的下拉框并入本页工具条；xlsx 已移除，见 F-17） */
const exportFormat = ref<'csv' | 'json'>('csv')
function doExport() {
  if (exportFormat.value === 'json') void exportJson()
  else void exportCsv()
}

/**
 * 图表容器的函数式 ref。
 *
 * 原先模板写 `ref="chartRef"`，而 chartRef 定义在 useHistoryQuery 内部且未在视图里解构，
 * 模板 ref 因此永远绑不上 → renderChart() 里 `if (!chartRef.value) return` 直接短路，
 * 页面只剩空图表框（有数据、有统计卡，但没有 canvas）。这里显式把元素写回 chartRef。
 */
function setChartEl(el: unknown) {
  chartRef.value = el instanceof HTMLElement ? el : undefined
}

// 查询条件与 URL 双向同步：切 Hub tab 返回后还原，且可刷新/分享带条件的链接
// （start/end 为秒级时间戳字符串，还原时转数字；granularity=raw 为默认不写入 URL）
const qsPointId = useQueryState('point_id')
const qsStart = useQueryState('start')
const qsEnd = useQueryState('end')
const qsDownsample = useQueryState('downsample')
const qsGranularity = useQueryState('granularity')
if (qsPointId.value) query.point_id = qsPointId.value
if (Number(qsStart.value) > 0) query.start = Number(qsStart.value)
if (Number(qsEnd.value) > 0) query.end = Number(qsEnd.value)
if (Number(qsDownsample.value) > 0) query.downsample = Number(qsDownsample.value)
if (qsGranularity.value === 'hour' || qsGranularity.value === 'day') granularity.value = qsGranularity.value
watch(
  () => [query.point_id, query.start, query.end, query.downsample, granularity.value] as const,
  ([pid, s, e, d, g]) => {
    qsPointId.value = pid
    qsStart.value = String(s ?? '')
    qsEnd.value = String(e ?? '')
    qsDownsample.value = d > 0 ? String(d) : ''
    qsGranularity.value = g === 'raw' ? '' : g
  },
)

// ===== 进页即出图：条件记忆 + 最近活跃点位兜底 =====
// 此前进页「点位 ID」为空、时间固定近 1 小时且不自动查询，核心链路首屏空转。
// 现在按「URL 条件 → 上次查询条件 → 最近活跃点位」三级回填，条件齐备即自动查一次；
// 5 分钟内同条件再次进入直接复用上次结果，避免来回切页时闪白重查。
const LAST_QUERY_KEY = 'history.last-query'
// 首次进入默认近 24 小时：多数网关的采集频率下，1 小时内命中数据的概率偏低
const DEFAULT_SPAN_MS = 86_400_000
const RESULT_TTL_MS = 5 * 60_000

/** 上次查询条件（按网关命名空间隔离，切网关不会串到别的网关点位） */
function readLastQuery(): { pointId: string; spanMs: number } | null {
  const raw = scopedGet(LAST_QUERY_KEY)
  if (!raw) return null
  try {
    const parsed = JSON.parse(raw) as { pointId?: unknown; spanMs?: unknown }
    const pointId = typeof parsed.pointId === 'string' ? parsed.pointId : ''
    if (!pointId) return null
    const spanMs = Number(parsed.spanMs)
    return { pointId, spanMs: Number.isFinite(spanMs) && spanMs > 0 ? spanMs : DEFAULT_SPAN_MS }
  } catch {
    return null
  }
}

/** 只记「点位 + 区间长度」：绝对时间下次进来必然过期，记住跨度的效果更好 */
function saveLastQuery(): void {
  const spanMs = Number(query.end) - Number(query.start)
  if (!query.point_id || spanMs <= 0) return
  try {
    scopedSet(LAST_QUERY_KEY, JSON.stringify({ pointId: query.point_id, spanMs }))
  } catch {
    // 隐私模式 / 配额满：仍可正常查询，只是不记忆
  }
}

/** 页面级结果缓存：仅本会话有效，换网关会整页 reload，不存在串台 */
type CachedResult = {
  at: number
  pointId: string
  spanMs: number
  multi: { pointId: string; data: HistoryRow[] }[]
  active: string
}
let resultCache: CachedResult | null = null

/** 未查询 / 无结果两态共用的引导文案（表格与图表复用同一句，避免两处口径不一致） */
const emptyHint = computed(() =>
  queried.value ? '该时段暂无数据，试试放宽时间范围' : '选择点位后点「查询」查看趋势',
)

/** 查询入口：查完同步记住条件并刷新缓存（模板的「查询」按钮走这里） */
async function runQuery(): Promise<void> {
  await queryHistory()
  if (!queried.value) return
  saveLastQuery()
  resultCache = {
    at: Date.now(),
    pointId: query.point_id,
    spanMs: Number(query.end) - Number(query.start),
    multi: multiHistory.value,
    active: activePointId.value,
  }
}

/** 无结果时一键放宽时间范围并重查 */
async function widenTo24h(): Promise<void> {
  setQuickRange(DEFAULT_SPAN_MS)
  await runQuery()
}

/** 命中缓存则不重复请求：条件一致、且距上次查询 < 5 分钟 */
function restoreCachedResult(): boolean {
  const c = resultCache
  if (!c || c.pointId !== query.point_id) return false
  const span = Number(query.end) - Number(query.start)
  if (Math.abs(c.spanMs - span) > 60_000 || Date.now() - c.at > RESULT_TTL_MS) return false
  multiHistory.value = c.multi
  activePointId.value = c.active
  queried.value = true
  void nextTick().then(() => renderChart())
  return true
}

onMounted(async () => {
  try {
    const list = await fetchPointBackoffOnce()
    pointCandidates.value = [...new Set(list.map((b) => b.point_id))].map((p) => ({ value: p }))
  } catch { /* 候选缺失不阻塞手输查询 */ }
  // 退避接口只列「正在退避」的点位，健康网关下为空；此时用全量点位质量接口兜底，
  // 否则候选与默认点位都会落空，首屏又回到「空表单等用户手输」。
  if (pointCandidates.value.length === 0) {
    try {
      const quality = await fetchPointQualities()
      pointCandidates.value = [...new Set(quality.map((p) => p.point_id))].map((p) => ({ value: p }))
    } catch { /* 同上：两路都失败时保持空表单 */ }
  }
  // 三路都空（新设备还没接入）时，用配置点位兜底：否则用户连点位 ID 都无从选起
  if (pointCandidates.value.length === 0) {
    await reloadPoints()
    pointCandidates.value = pointOptions.value.map((p) => ({ value: p.id }))
  }

  // 分享链接 / 刷新带了条件时以 URL 为准，不覆盖用户明确指定的查询
  if (!qsPointId.value) {
    const last = readLastQuery()
    if (last) {
      query.point_id = last.pointId
      setQuickRange(last.spanMs)
    }
  }

  // 仍无点位：取最近活跃点位，让首屏直接出图，而不是空表单 + 一句「请先输入」
  if (!query.point_id) {
    query.point_id = pointCandidates.value[0]?.value ?? ''
    if (query.point_id) setQuickRange(DEFAULT_SPAN_MS)
  }
  // 一个点位都拿不到（全新空网关）：保持空表单，引导文案会提示手选
  if (!query.point_id) return

  if (restoreCachedResult()) return
  await runQuery()
})

// A4: DataTable 列定义。cell 直接拿到整行，保留原 el-table-column 的渲染效果。
const historyColumns = computed<ColumnDef<HistoryRow>[]>(() => [
  {
    key: 'timestamp',
    title: '时间',
    width: 200,
    cell: (row) => h('span', formatTime(row.timestamp)),
  },
  {
    key: 'value',
    title: '值',
    width: 150,
    cell: (row) => h('span', { class: 'value-cell' }, String(row.value)),
  },
  {
    key: 'quality',
    title: '质量',
    width: 110,
    cell: (row) =>
      h(
        ElTag,
        { type: qualityTagType(row.quality), effect: 'dark', size: 'small', round: true },
        () => qualityText(row.quality)
      ),
  },
])
</script>

<style scoped src="./History.css"></style>
