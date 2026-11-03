<template>
  <div class="cp3">
    <SectionHeader title="通信画像" icon="DataAnalysis">
      <el-button size="small" @click="goHealthScore">查看设备健康评分</el-button>
    </SectionHeader>
    <el-card shadow="never" class="section-card">
      <template #header>
        <div class="card-head">
          <strong>各设备通信画像概览</strong>
          <div class="head-actions">
            <!-- N-5：通信矩阵与本页同源（同一 stats-report 接口、同一设备指标），
                 合并为一页两种形态，避免同一份数据出现两个真相。 -->
            <el-radio-group v-model="viewMode" size="small">
              <el-radio-button value="table">表格</el-radio-button>
              <el-radio-button value="matrix">矩阵</el-radio-button>
            </el-radio-group>
            <el-button size="small" link @click="load">刷新</el-button>
            <TablePrefsMenu
              v-if="viewMode === 'table'"
              table-id="comm-profile"
              :columns="profileColumns.map(c => ({ key: c.key, title: c.title }))"
            />
          </div>
        </div>
      </template>

      <DataTable
        v-if="viewMode === 'table'"
        :data="profiles"
        :columns="profileColumns"
        :hidden-columns="hiddenColumns"
        row-key="deviceId"
        stripe
        :loading="loading"
        empty-text="暂无画像数据，需先让总线上的设备产生采集流量"
      />

      <div v-else class="matrix-wrap">
        <p class="matrix-caption">矩阵 [行] 设备 [列] 总线 采集成功率</p>
        <div v-if="profiles.length === 0" class="matrix-empty">暂无矩阵数据，需先让总线上的设备产生采集流量</div>
        <template v-else>
          <div class="matrix">
            <div class="m-header">
              <div class="m-cell m-label">设备 \ 总线</div>
              <div v-for="b in buses" :key="b" class="m-cell m-bus">{{ b }}</div>
            </div>
            <div v-for="p in profiles" :key="p.deviceId" class="m-row">
              <div class="m-cell m-label">{{ p.deviceName }}</div>
              <div
                v-for="b in buses"
                :key="b"
                class="m-cell m-data"
                :style="cellStyle(p, b)"
                :title="cellTitle(p, b)"
              >
                <span class="sr">{{ cellRate(p, b) !== null ? cellRate(p, b) + '%' : '-' }}</span>
              </div>
            </div>
          </div>
          <div class="legend">
            <span>低</span>
            <span class="bar"><i style="background:var(--el-color-danger)"></i><i style="background:var(--el-color-warning)"></i><i style="background:var(--el-color-success-light-3)"></i><i style="background:var(--el-color-success)"></i></span>
            <span>高</span>
          </div>
        </template>
      </div>
    </el-card>
    <!-- W2-#14：与「设备健康评分」统一口径说明（两页都按设备给通信健康度，但不是同一个数） -->
    <el-alert
      title="口径说明：本页「通信评级」≠「设备健康评分」"
      type="info"
      :closable="false"
      show-icon
    >
      <div class="caliber">
        <div>• 通信评级：只按<strong>采集成功率</strong>分四档（≥98 优秀 / ≥93 良好 / ≥80 一般 / &lt;80 较差），不叠加延迟与离线权重。</div>
        <div>• 设备健康评分：见「设备健康评分」页，按 24h 成功率 + 平均延迟 + 失败次数 + 离线次数加权成 0–100 分（≥90 优秀 / ≥70 良好 / ≥50 告警 / &lt;50 异常）。</div>
        <div>• 两页的「采集成功率」「平均延迟」取数口径一致；评级与综合评分不是等价指标，不能互相替代。</div>
      </div>
    </el-alert>
  </div>
</template>

<script setup lang="ts">
import { h, ref, onMounted } from 'vue'
import { useRouter } from 'vue-router'
import { ElProgress, ElTag } from 'element-plus'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import type { ColumnDef } from '@/components/DataTable.vue'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
import { type DeviceStatView } from '@/api/diagnostics'
import { useStatsReport } from '@/composables/data/useStatsReport'
import { deviceFailureMap, fetchPointQualities } from '@/composables/data/usePointQuality'
import { getConfig } from '@/api/config'
import { communicationRating, communicationRateStatus } from '@/utils/healthVerdict'

interface CommProfile {
  deviceId: string;
  deviceName: string;
  /** 设备所属总线名；矩阵形态按此分列，缺省回退协议名 */
  busName: string;
  /** null = 报表窗口内无采集 */
  avgRespMs: number | null;
  successRate: number | null;
  lossRate: number | null;
  timeoutCount: number;
  /** 该设备下连续失败点位数之和（原「时延分析」页的核心指标，合并后在此展示） */
  consecutiveFailures: number;
  currentPeriodMs: number;
  suggestedPeriodMs: number;
  suggestion: string;
  health: 'excellent' | 'good' | 'fair' | 'poor' | 'unknown';
}

const HEALTH_LABEL: Record<string, string> = { excellent: '优秀', good: '良好', fair: '一般', poor: '较差', unknown: '未采集' }
const HEALTH_COLOR: Record<CommProfile['health'], 'success' | 'warning' | 'danger' | 'info'> = { excellent: 'success', good: 'success', fair: 'warning', poor: 'danger', unknown: 'info' }

// 通信评级档位统一由 utils/healthVerdict 提供：与成功率进度条同源，
// 避免「评级优秀但进度条告警」；与「设备健康评分」仍是不同指标。
const healthFromRate = communicationRating

const router = useRouter()
/** 双向跳转（W2-#14）：本页给通信侧评级，综合分与离线维度去健康评分页看 */
function goHealthScore() {
  router.push('/devices?tab=health-score')
}

const profiles = ref<CommProfile[]>([])
const buses = ref<string[]>([])
const viewMode = ref<'table' | 'matrix'>('table')
const loading = ref(false)
// 报表取数统一走 useStatsReport（单一数据入口）
const { load: loadStats } = useStatsReport()

// 表格统一走 DataTable：通信评级/延迟/成功率等列改为 cell 渲染，列宽/列顺序与迁移前一致
const profileColumns: ColumnDef<CommProfile>[] = [
  { key: 'deviceName', title: '设备', minWidth: 130 },
  {
    key: 'health',
    title: '通信评级',
    width: 100,
    cell: (row) => h(ElTag, { size: 'small', effect: 'dark', round: true, type: HEALTH_COLOR[row.health] }, () => HEALTH_LABEL[row.health]),
  },
  { key: 'avgRespMs', title: '平均延迟(ms)', width: 130, align: 'right', cell: (row) => h('div', { class: 'resp' }, row.avgRespMs == null ? '—' : `${row.avgRespMs} ms`) },
  {
    key: 'successRate',
    title: '采集成功率',
    width: 130,
    // 窗口内无采集：显示「—」，不画 0% 的空进度条。阈值与「通信评级」同一口径
    cell: (row) =>
      row.successRate == null
        ? h('span', { class: 'text-muted' }, '—')
        : h(ElProgress, {
            percentage: row.successRate,
            status: communicationRateStatus(row.successRate),
            strokeWidth: 10,
          }),
  },
  { key: 'lossRate', title: '丢包率(%)', width: 100, align: 'right', cell: (row) => (row.lossRate == null ? '—' : row.lossRate) },
  { key: 'timeoutCount', title: '采集失败次数', width: 110, align: 'right' },
  {
    key: 'consecutiveFailures',
    title: '点位连续失败',
    width: 120,
    align: 'right',
    cell: (row) =>
      row.consecutiveFailures > 0
        ? h(ElTag, { size: 'small', type: 'danger', effect: 'plain' }, () => String(row.consecutiveFailures))
        : String(row.consecutiveFailures),
  },
  { key: 'currentPeriodMs', title: '当前周期', width: 110, align: 'right', cell: (row) => `${row.currentPeriodMs}ms` },
  { key: 'suggestedPeriodMs', title: '建议周期', width: 110, align: 'right', cell: (row) => h('span', { class: 'suggest' }, `${row.suggestedPeriodMs}ms`) },
  { key: 'suggestion', title: '优化建议', minWidth: 260, ellipsis: true },
]

const { hidden: hiddenColumns } = useColumnPrefs('comm-profile')

// ---------- 矩阵形态（原「通信矩阵」页签，与表格共用同一份数据） ----------

function cellRate(p: CommProfile, bus: string): number | null {
  return p.busName === bus ? p.successRate : null
}

function successColor(rate: number): string {
  // 与「通信评级」四档同一口径：≥98 优秀 / ≥93 良好 / ≥80 一般 / <80 较差
  if (rate >= 98) return 'var(--color-success)'
  if (rate >= 93) return 'var(--color-success-light)'
  if (rate >= 80) return 'var(--color-warning)'
  return 'var(--color-danger)'
}

function cellStyle(p: CommProfile, bus: string) {
  const r = cellRate(p, bus)
  if (r === null) return { background: 'var(--el-fill-color-light)' }
  return { background: successColor(r) }
}

function cellTitle(p: CommProfile, bus: string) {
  const rate = cellRate(p, bus)
  if (rate === null) return '未知设备'
  const latency = p.avgRespMs == null ? '—' : `${p.avgRespMs}ms`
  return `${p.deviceName} @ ${bus}\n成功率: ${rate}%\n平均延迟: ${latency}`
}

async function load() {
  loading.value = true
  try {
    const [cfg, report, qualities] = await Promise.all([getConfig(), loadStats(), fetchPointQualities()])
    buses.value = (cfg.buses || []).map((b: { port_name: string }) => b.port_name)
    const devBusMap = new Map<string, string>()
    for (const d of cfg.devices || []) devBusMap.set(d.id, d.bus)
    // 连续失败点位：按设备累加点位质量里的 consecutive_failures（原「时延分析」页同口径）
    const failureMap = deviceFailureMap(qualities)
    const pointIntervalMap = new Map<string, number>()
    for (const pt of cfg.points) {
      if (pt.device_id) {
        const cur = pointIntervalMap.get(pt.device_id) ?? Infinity
        pointIntervalMap.set(pt.device_id, Math.min(cur, pt.interval))
      }
    }
    profiles.value = report.deviceStats.map((d: DeviceStatView) => {
      const currentMs = (pointIntervalMap.get(d.deviceId) ?? 1) * 1000
      const health = healthFromRate(d.successRate)
      const suggested = health === 'excellent' ? Math.max(500, currentMs / 2)
        : health === 'good' ? currentMs
        : health === 'fair' ? currentMs * 2
        : health === 'poor' ? currentMs * 3
        : currentMs
      const lossRate = d.successRate == null ? null : Math.round((100 - d.successRate) * 10) / 10
      let suggestion = ''
      if (health === 'unknown') suggestion = '统计窗口内无采集数据，无法评估'
      else if (health === 'excellent') suggestion = `响应稳定(${d.avgLatency}ms)，可考虑缩短采集周期`
      else if (health === 'good') suggestion = '通信良好，建议保持当前周期并关注抖动'
      else if (health === 'fair') suggestion = `丢包率${lossRate}%，建议延长周期并检查线缆`
      else suggestion = `通信质量差，建议大幅延长周期至${Math.round(suggested)}ms并排查设备负载`
      return {
        deviceId: d.deviceId, deviceName: d.deviceName,
        busName: devBusMap.get(d.deviceId) || d.protocol,
        avgRespMs: d.avgLatency, successRate: d.successRate,
        lossRate, timeoutCount: d.failed,
        consecutiveFailures: failureMap.get(d.deviceId) || 0,
        currentPeriodMs: currentMs, suggestedPeriodMs: Math.round(suggested),
        suggestion, health,
      }
    })
  } catch (e) {
    showToast(e, '加载通信画像失败')
  } finally {
    loading.value = false
  }
}

onMounted(load)
</script>

<style scoped>
.cp3 { padding: 16px; }
.section-card { margin-bottom: 16px; }
.caliber { font-size: 0.8125rem; line-height: 1.7; }
.card-head { display: flex; justify-content: space-between; align-items: center; gap: 12px; }
.head-actions { display: flex; align-items: center; gap: 12px; }
.resp { font-size: 0.8125rem; }
.text-muted { color: var(--text-secondary); }
.suggest { color: var(--el-color-primary); font-weight: 600; }

.matrix-wrap { min-height: 120px; }
.matrix-caption { margin: 0 0 10px; font-size: 0.8125rem; color: var(--text-secondary); }
.matrix-empty { padding: 40px 0; text-align: center; color: var(--text-secondary); font-size: 0.875rem; }
.matrix { display: inline-block; }
.m-header, .m-row { display: flex; }
.m-cell { min-width: 120px; height: 44px; display: flex; align-items: center; justify-content: center; border: 1px solid var(--el-border-color-lighter); }
.m-label { min-width: 120px; background: var(--el-fill-color-light); font-weight: 600; color: var(--el-text-color-regular); }
.m-bus { background: var(--el-fill-color-light); color: var(--el-text-color-secondary); font-weight: 600; }
.m-data { cursor: default; }
.sr { color: var(--text-inverse); font-weight: 600; font-size: 0.875rem; text-shadow: 0 1px 2px rgba(0, 0, 0, 0.3); }
.legend { display: flex; align-items: center; gap: 8px; margin-top: 12px; font-size: 0.75rem; color: var(--el-text-color-secondary); justify-content: flex-end; }
.legend .bar { display: flex; gap: 2px; }
.legend .bar i { display: inline-block; width: 40px; height: 12px; border-radius: var(--radius-bar); }
</style>
