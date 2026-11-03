<template>
  <div class="health">
    <SectionHeader title="设备健康评分" icon="FirstAidKit">
      <el-radio-group v-model="sortBy" size="small" style="margin-right: 8px">
        <el-radio-button value="score">综合评分</el-radio-button>
        <el-radio-button value="successRate">采集成功率</el-radio-button>
        <el-radio-button value="latency">平均延迟</el-radio-button>
        <el-radio-button value="failures">连续失败</el-radio-button>
      </el-radio-group>
      <el-button size="small" @click="goCommProfile">查看通信画像</el-button>
      <el-button @click="refresh"><el-icon><Refresh /></el-icon> 刷新评分</el-button>
      <TablePrefsMenu table-id="health-score" :columns="healthColumns.map(c => ({ key: c.key, title: c.title }))" />
      </SectionHeader>

    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="评分口径：本页是设备综合健康评分的唯一来源（24h 成功率 / 平均延迟 / 失败次数 / 累计离线次数加权成 0–100 分）。"
      style="margin-bottom:12px"
    >
      <div class="caliber">
        <div>• 分档：≥90 优秀 / ≥70 良好 / ≥50 告警 / &lt;50 异常 / 无数据未采集。列表可按维度重排，分值本身不变。</div>
        <div>• 与「通信画像」的区别：画像页的<strong>通信评级</strong>只按采集成功率分四档（≥98/≥93/≥80/&lt;80），不含延迟与离线权重，两者不是等价指标。</div>
        <div>• 两页的「采集成功率」「平均延迟」取数口径一致；点位级质量码明细见「数据质量」。</div>
      </div>
    </el-alert>

    <!-- 总体概览 -->
    <div class="overview">
      <div class="score-ring" :class="levelClass(summary.avgScore)">
        <svg viewBox="0 0 100 100">
          <circle cx="50" cy="50" r="42" fill="none" stroke="var(--border-light)" stroke-width="8" />
          <circle cx="50" cy="50" r="42" fill="none" :stroke="scoreColor(summary.avgScore)" stroke-width="8"
            stroke-linecap="round" :stroke-dasharray="264" :stroke-dashoffset="264 - (264 * (summary.avgScore ?? 0) / 100)"
            transform="rotate(-90 50 50)" />
        </svg>
        <div class="ring-center">
          <div class="score-num">{{ summary.avgScore ?? '—' }}</div>
          <div class="score-label">平台设备平均健康分</div>
        </div>
      </div>
      <div class="overview-stats">
        <div class="stat-item excellent">
          <div class="stat-num">{{ summary.excellent }}</div>
          <div class="stat-label">优秀 (≥90)</div>
        </div>
        <div class="stat-item good">
          <div class="stat-num">{{ summary.good }}</div>
          <div class="stat-label">良好 (≥70)</div>
        </div>
        <div class="stat-item warning">
          <div class="stat-num">{{ summary.warning }}</div>
          <div class="stat-label">告警 (≥50)</div>
        </div>
        <div class="stat-item critical">
          <div class="stat-num">{{ summary.critical }}</div>
          <div class="stat-label">异常 (&lt;50)</div>
        </div>
        <div class="stat-item unknown">
          <div class="stat-num">{{ summary.unknown }}</div>
          <div class="stat-label">未采集</div>
        </div>
      </div>
    </div>

    <!-- 设备健康列表 -->
    <div style="margin-top: 16px">
      <DataTable
        :data="healthList"
        :columns="healthColumns"
        :hidden-columns="hiddenColumns"
        row-key="deviceId"
        stripe
        :loading="loading"
        empty-text="暂无评分，需设备先产生采集与在线记录"
      />
    </div>

    <!-- R3：设备影子并入本页，作为「期望 vs 实测」子表（原「设备状态快照」页签已下线）。
         健康评分看 24h 长期表现，本表看当前快照，两者同源于采集结果，放在一起便于对照。 -->
    <div class="expected-actual">
      <SectionHeader title="期望 vs 实测（设备状态快照）" icon="Box">
        <el-button size="small" :loading="shadowLoading" @click="loadShadows">
          <el-icon><Refresh /></el-icon> 刷新快照
        </el-button>
        <TablePrefsMenu table-id="device-expected-actual" :columns="shadowColumns.map(c => ({ key: c.key, title: c.title }))" />
      </SectionHeader>
      <el-alert
        type="info"
        :closable="false"
        show-icon
        title="期望周期取自设备配置的采集间隔；相对期望按「最后上报距今是否超过 2 个期望周期」判定。"
        description="本页上半部分是 24h 综合评分（长期表现），本表是最近一次上报快照（当前状态），两者不互为替代。"
        style="margin-bottom:12px"
      />
      <DataTable
        :data="shadows"
        :columns="shadowColumns"
        :hidden-columns="hiddenShadowColumns"
        row-key="deviceId"
        stripe
        :loading="shadowLoading"
        empty-text="暂无快照数据，设备首次上报采集值后自动生成"
      />
    </div>

    <!-- 健康详情抽屉 -->
    <el-drawer v-model="showDetailDrawer" title="设备健康详情" size="36%">
      <div v-if="current" class="detail">
        <div class="detail-score" :class="levelClass(current.score)">
          <div class="detail-score-num">{{ current.score ?? '—' }}</div>
          <div class="detail-score-label">{{ levelLabel(current.level) }}</div>
        </div>
        <el-descriptions :column="1" border style="margin-top: 16px">
          <el-descriptions-item label="设备">{{ current.deviceName }}</el-descriptions-item>
          <el-descriptions-item label="协议">{{ current.protocol }}</el-descriptions-item>
          <el-descriptions-item label="所属总线">{{ current.bus || '—' }}</el-descriptions-item>
          <el-descriptions-item label="采集成功率">{{ pct(current.successRate) }}</el-descriptions-item>
          <el-descriptions-item label="平均延迟">{{ ms(current.avgLatency) }}</el-descriptions-item>
          <el-descriptions-item label="错误数">{{ current.errorCount }} 次</el-descriptions-item>
          <el-descriptions-item label="离线次数">{{ current.offlineCount }} 次</el-descriptions-item>
          <el-descriptions-item label="好质量点位占比">{{ pct(current.goodPointRatio) }}</el-descriptions-item>
          <el-descriptions-item label="点位连续失败">{{ current.consecutiveFailures }} 次</el-descriptions-item>
          <el-descriptions-item label="质量码分布">
            好 {{ current.qualityBreakdown.good }} / 不确定 {{ current.qualityBreakdown.uncertain }} /
            坏 {{ current.qualityBreakdown.bad }} / 未知 {{ current.qualityBreakdown.unknown }}
            （共 {{ current.qualityBreakdown.total }}）
          </el-descriptions-item>
        </el-descriptions>
        <h4 style="margin-top: 20px">趋势详情</h4>
        <div v-for="d in current.details" :key="d.label" class="detail-item">
          <div class="detail-item-head">
            <span>{{ d.label }}</span>
            <span>{{ d.value }}</span>
          </div>
          <el-progress :percentage="d.score" :color="scoreColor(d.score)" :show-text="false" :stroke-width="8" />
        </div>
      </div>
    </el-drawer>

    <!-- 快照点位抽屉：原「设备状态快照」页的逐点最新值能力，收进本页的按需查看 -->
    <el-drawer v-model="showShadowDrawer" title="设备状态快照 · 点位明细" size="46%">
      <div v-if="currentShadow">
        <el-descriptions :column="2" border size="small">
          <el-descriptions-item label="设备">{{ currentShadow.deviceName }}</el-descriptions-item>
          <el-descriptions-item label="协议">{{ currentShadow.protocol }}</el-descriptions-item>
          <el-descriptions-item label="期望周期">{{ formatInterval(currentShadow.expectedIntervalMs) }}</el-descriptions-item>
          <el-descriptions-item label="最后上报">{{ timeAgo(currentShadow.lastUpdateAt > 0 ? currentShadow.lastUpdateAt : null) }}</el-descriptions-item>
          <el-descriptions-item label="最近值提示" :span="2">{{ currentShadow.lastValueHint || '—' }}</el-descriptions-item>
        </el-descriptions>
        <div style="margin-top: 16px">
          <DataTable
            :data="currentShadow.points"
            :columns="shadowPointColumns"
            row-key="id"
            stripe
            empty-text="该设备暂无点位快照"
          />
        </div>
      </div>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { h, ref, computed, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElButton, ElIcon, ElProgress, ElTag } from 'element-plus'
import {Refresh, Cpu} from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import {
  deviceScoreColor,
  deviceScoreTextColor,
  deviceScoreLevel,
  DEVICE_SCORE_LEVEL_LABEL,
} from '@/utils/healthVerdict'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
import {
  listHealthView,
  listShadowsView,
  summarizeHealth,
  qualityLabel,
  type DeviceHealthView,
  type DeviceShadowView,
  type DeviceShadowPointView,
  type HealthSummaryView,
} from '@/api/deviceOps'

/** 最近一次拉取的完整列表（排序前的原始顺序） */
const healthRows = ref<DeviceHealthView[]>([])
const summary = ref<HealthSummaryView>({ total: 0, excellent: 0, good: 0, warning: 0, critical: 0, unknown: 0, avgScore: null })
const showDetailDrawer = ref(false)
const current = ref<DeviceHealthView | null>(null)
const loading = ref(false)
/** R3：设备影子（期望周期 vs 实测快照）并入本页，不再单独占页签 */
const shadows = ref<DeviceShadowView[]>([])
const shadowLoading = ref(false)
const showShadowDrawer = ref(false)
const currentShadow = ref<DeviceShadowView | null>(null)
/**
 * 排序维度：原「质量排行」页只是同一份数据的可排序视图，已合并到本页。
 * 无数据的设备（score/successRate/latency 为 null）始终排在最后，不用 0 分冒充。
 */
const sortBy = ref<'score' | 'successRate' | 'latency' | 'failures'>('score')
const healthList = computed<DeviceHealthView[]>(() => {
  const list = [...healthRows.value]
  if (sortBy.value === 'successRate') {
    list.sort((a, b) => (b.successRate ?? -1) - (a.successRate ?? -1))
  } else if (sortBy.value === 'latency') {
    list.sort((a, b) => (a.avgLatency ?? Number.MAX_SAFE_INTEGER) - (b.avgLatency ?? Number.MAX_SAFE_INTEGER))
  } else if (sortBy.value === 'failures') {
    list.sort((a, b) => a.consecutiveFailures - b.consecutiveFailures)
  } else {
    list.sort((a, b) => (b.score ?? -1) - (a.score ?? -1))
  }
  return list
})

// 评分阈值与颜色统一由 utils/healthVerdict 提供（与通信画像、健康报告同源）
const scoreColor = deviceScoreColor
const scoreTextColor = deviceScoreTextColor
const levelClass = deviceScoreLevel

function levelTag(l: DeviceHealthView['level']): 'success' | 'primary' | 'warning' | 'danger' | 'info' {
  return ({ excellent: 'success', good: 'primary', warning: 'warning', critical: 'danger', unknown: 'info' } as const)[l]
}

// 后端对「24h 内无采集 / 从未在线」返回 null，统一渲染「—」，不用 0 冒充
const pct = (v: number | null) => (v == null ? '—' : `${v}%`)
const ms = (v: number | null) => (v == null ? '—' : `${v} ms`)

// 表格统一走 DataTable；列顺序/宽度与迁移前保持一致，超阈值自动切虚拟滚动
const healthColumns: ColumnDef<DeviceHealthView>[] = [
  {
    key: 'deviceName',
    title: '设备',
    minWidth: 160,
    cell: (row) =>
      h('div', { class: 'device-name' }, [
        h(ElIcon, null, () => h(Cpu)),
        h('span', null, row.deviceName),
        h(ElTag, { size: 'small', type: 'info', effect: 'plain' }, () => row.protocol),
      ]),
  },
  {
    key: 'score',
    title: '健康评分',
    width: 180,
    cell: (row) =>
      row.score == null
        ? h('span', { class: 'text-muted' }, '—')
        : h('div', { class: 'score-bar' }, [
            h(ElProgress, { percentage: row.score, color: scoreColor(row.score), showText: false, strokeWidth: 10 }),
            h('span', { class: 'score-text', style: { color: scoreTextColor(row.score) } }, String(row.score)),
          ]),
  },
  {
    key: 'level',
    title: '等级',
    width: 100,
    cell: (row) => h(ElTag, { type: levelTag(row.level), effect: 'dark', round: true }, () => levelLabel(row.level)),
  },
  { key: 'successRate', title: '采集成功率', width: 120, cell: (row) => pct(row.successRate) },
  { key: 'avgLatency', title: '平均延迟', width: 110, cell: (row) => ms(row.avgLatency) },
  {
    key: 'errorCount',
    title: '错误数',
    width: 80,
    cell: (row) => h('span', { class: { 'text-danger': row.errorCount > 0 } }, String(row.errorCount)),
  },
  {
    key: 'offlineCount',
    title: '离线次数',
    width: 80,
    cell: (row) => h('span', { class: { 'text-danger': row.offlineCount > 0 } }, String(row.offlineCount)),
  },
  { key: 'lastSeenAt', title: '最后在线', width: 110, cell: (row) => timeAgo(row.lastSeenAt) },
  actionColumn<DeviceHealthView>({
    width: 80,
    cell: (row) => h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => showDetail(row) }, () => '查看详情'),
  }),
]

/** 快照质量标签色：与全站质量口径一致（good=良好 / uncertain=不确定 / 其余按坏值） */
type QualityTagType = 'success' | 'warning' | 'danger' | 'info'

function shadowQualityTag(q: string): QualityTagType {
  if (q === 'good') return 'success'
  if (q === 'uncertain') return 'warning'
  return 'danger'
}

function lastQualityText(q: string): string {
  return q === 'good' ? '良好' : q === 'uncertain' ? '不确定' : '异常'
}

/** 期望周期 → 可读文本（秒级优先，避免 5000ms 这类裸数字） */
function formatInterval(ms: number): string {
  if (!Number.isFinite(ms) || ms <= 0) return '—'
  return ms >= 1000 ? `${Number((ms / 1000).toFixed(2))} 秒` : `${ms} ms`
}

/** 期望 vs 实测：最后上报距今超过 2 个期望周期即视为超期（0 = 从未上报，不冒充正常） */
function freshness(row: DeviceShadowView): { text: string; type: QualityTagType } {
  if (row.lastUpdateAt <= 0) return { text: '无上报', type: 'info' }
  if (row.expectedIntervalMs <= 0) return { text: '—', type: 'info' }
  return Date.now() - row.lastUpdateAt > row.expectedIntervalMs * 2
    ? { text: '已超期', type: 'warning' }
    : { text: '在期望内', type: 'success' }
}

function pointQualityText(row: DeviceShadowView): string {
  if (!row.points.length) return '—'
  const counts = { good: 0, uncertain: 0, bad: 0 }
  for (const p of row.points) counts[p.quality]++
  return `好 ${counts.good} / 不确定 ${counts.uncertain} / 坏 ${counts.bad}`
}

const shadowColumns: ColumnDef<DeviceShadowView>[] = [
  {
    key: 'deviceName',
    title: '设备',
    minWidth: 160,
    cell: (row) =>
      h('div', { class: 'device-name' }, [
        h(ElIcon, null, () => h(Cpu)),
        h('span', null, row.deviceName),
        h(ElTag, { size: 'small', type: 'info', effect: 'plain' }, () => row.protocol),
      ]),
  },
  { key: 'expectedIntervalMs', title: '期望周期', width: 110, cell: (row) => formatInterval(row.expectedIntervalMs) },
  {
    key: 'lastUpdateAt',
    title: '最后上报',
    width: 110,
    cell: (row) => timeAgo(row.lastUpdateAt > 0 ? row.lastUpdateAt : null),
  },
  {
    key: 'freshness',
    title: '相对期望',
    width: 110,
    cell: (row) => {
      const f = freshness(row)
      return h(ElTag, { size: 'small', type: f.type, effect: 'plain' }, () => f.text)
    },
  },
  {
    key: 'lastQuality',
    title: '最近质量',
    width: 110,
    cell: (row) =>
      h(
        ElTag,
        { size: 'small', type: shadowQualityTag(row.lastQuality), effect: 'dark', round: true },
        () => lastQualityText(row.lastQuality),
      ),
  },
  { key: 'points', title: '点位质量（好/不确定/坏）', minWidth: 180, cell: (row) => pointQualityText(row) },
  actionColumn<DeviceShadowView>({
    width: 90,
    cell: (row) => h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => showShadow(row) }, () => '查看点位'),
  }),
]

const shadowPointColumns: ColumnDef<DeviceShadowPointView>[] = [
  { key: 'id', title: '点位', minWidth: 160 },
  { key: 'lastValue', title: '最新值', minWidth: 120, cell: (row) => row.lastValue ?? '—' },
  {
    key: 'quality',
    title: '质量',
    width: 100,
    cell: (row) => h(ElTag, { size: 'small', type: shadowQualityTag(row.quality), effect: 'plain' }, () => qualityLabel(row.quality)),
  },
]

const { hidden: hiddenColumns } = useColumnPrefs('health-score')
const { hidden: hiddenShadowColumns } = useColumnPrefs('device-expected-actual')

/** 与「通信画像」双向跳转：本页看综合分，画像页看逐设备通信明细与参数 */
const router = useRouter()
function goCommProfile() {
  router.push('/acquisition/comm-tuning?tab=comm-profile')
}

function levelLabel(l: DeviceHealthView['level']) {
  return DEVICE_SCORE_LEVEL_LABEL[l]
}

function timeAgo(t: number | null) {
  if (t == null) return '—'
  const s = Math.floor((Date.now() - t) / 1000)
  if (s < 60) return `${s}秒前`
  if (s < 3600) return `${Math.floor(s / 60)}分钟前`
  return `${Math.floor(s / 3600)}小时前`
}

async function load() {
  loading.value = true
  try {
    // 第 1 期冗余收口：`/api/devices/health/summary` 已删除，
    // 汇总由列表派生（与列表同一份快照，不会读到两个不一致的时点）。
    const list = await listHealthView()
    healthRows.value = list
    summary.value = summarizeHealth(list)
  } catch (e) {
    showToast(e, '加载健康评分失败')
  } finally {
    loading.value = false
  }
}

async function refresh() {
  // 后端实时计算，刷新即重新拉数据；评分与快照同一次刷新，避免两个时点的数据并排展示
  await Promise.all([load(), loadShadows()])
}

/** 拉取设备影子（期望 vs 实测子表）：与健康评分同源，但统计口径是「最近一次上报」 */
async function loadShadows() {
  shadowLoading.value = true
  try {
    shadows.value = await listShadowsView()
  } catch (e) {
    showToast(e, '加载设备状态快照失败')
  } finally {
    shadowLoading.value = false
  }
}

function showShadow(row: DeviceShadowView) {
  currentShadow.value = row
  showShadowDrawer.value = true
}

function showDetail(h: DeviceHealthView) {
  current.value = h
  showDetailDrawer.value = true
}

onMounted(() => {
  void load()
  void loadShadows()
})
</script>

<style scoped src="./HealthScore.css"></style>
