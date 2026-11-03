<template>
  <div class="report">
    <SectionHeader title="采集统计" icon="DataLine" />

    <div class="top-bar">
      <el-radio-group v-model="period" @change="load">
        <el-radio-button value="day">日报</el-radio-button>
        <el-radio-button value="week">周报</el-radio-button>
        <el-radio-button value="month">月报</el-radio-button>
      </el-radio-group>
      <el-button type="primary" :disabled="!report" @click="exportCsv">
        <el-icon><Download /></el-icon> 导出 CSV
      </el-button>
      <el-button @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
      <TablePrefsMenu table-id="stats-report" :columns="deviceColumns.map(c => ({ key: c.key, title: c.title }))" />
    </div>

    <div v-if="report" class="report-body">
      <!-- 总览卡片 -->
      <div class="summary-cards">
        <StatCard label="总采集次数" :value="report.summary.totalReads.toLocaleString()" />
        <StatCard label="采集成功率" :value="fmtPct(report.summary.successRate)" color="var(--color-success)" />
        <StatCard label="平均延迟" :value="fmtMs(report.summary.avgLatency)" unit="" />
        <StatCard label="数据点数" :value="report.summary.totalDataPoints.toLocaleString()" />
      </div>

      <!-- 设备统计 -->
      <el-card shadow="never" class="section-card" style="margin-top: 16px">
        <template #header>
          <SectionHeader title="按设备统计" icon="Cpu" />
        </template>
        <DataTable
          :data="report.deviceStats"
          :columns="deviceColumns"
          :hidden-columns="hiddenColumns"
          row-key="deviceId"
          stripe
          empty-text="暂无设备采集统计"
        />
      </el-card>

      <!-- 总线统计 -->
      <el-card shadow="never" class="section-card" style="margin-top: 16px">
        <template #header>
          <SectionHeader title="按总线统计" icon="Connection" />
        </template>
        <DataTable
          :data="report.busStats"
          :columns="busColumns"
          row-key="bus"
          stripe
          empty-text="暂无总线采集统计"
        />
      </el-card>

      <!-- 错误分布 -->
      <el-card shadow="never" class="section-card" style="margin-top: 16px">
        <template #header>
          <SectionHeader title="采集错误类型分布" icon="Warning" />
        </template>
        <div class="error-hint">统计口径：进程启动以来的累计错误（非所选报表周期）</div>
        <EmptyState v-if="report.errorDist.length === 0" description="暂无采集错误记录" />
        <div v-else class="error-dist">
          <div v-for="e in report.errorDist" :key="e.errorType" class="error-bar">
            <div class="error-name">{{ e.errorType }}</div>
            <div class="error-track">
              <div class="error-fill" :style="{ width: e.percentage + '%' }"></div>
            </div>
            <div class="error-count">{{ e.count }} 次 ({{ e.percentage }}%)</div>
          </div>
        </div>
      </el-card>
    </div>
  </div>
</template>

<script setup lang="ts">
import { h, onMounted } from 'vue'
import { ElMessage, ElProgress, ElTag } from 'element-plus'
import {Download, Refresh} from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import { CHART_SUCCESS, CHART_WARNING, CHART_DANGER } from '@/utils/chartTheme'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
import StatCard from '@/components/StatCard.vue'
import EmptyState from '@/components/EmptyState.vue'
import {
  exportStatsReportCsv as exportReportCsv,
  type DeviceStatView,
  type BusStatView,
} from '@/api/diagnostics'
import { useStatsReport } from '@/composables/data/useStatsReport'

// 表格统一走 DataTable；列顺序/宽度与迁移前保持一致
const deviceColumns: ColumnDef<DeviceStatView>[] = [
  { key: 'deviceName', title: '设备', width: 120 },
  { key: 'protocol', title: '协议', width: 90, cell: (row) => h(ElTag, { size: 'small', effect: 'plain' }, () => row.protocol) },
  { key: 'totalReads', title: '总采集', width: 110, align: 'right' },
  { key: 'success', title: '成功', width: 100, align: 'right' },
  {
    key: 'failed',
    title: '失败',
    width: 100,
    align: 'right',
    cell: (row) => h('span', { class: { 'text-danger': row.failed > 0 } }, String(row.failed)),
  },
  {
    key: 'successRate',
    title: '成功率',
    width: 140,
    // 窗口内无采集时后端返回 null：显示「—」，不画 0% 的空进度条
    cell: (row) =>
      row.successRate == null
        ? h('span', { class: 'text-muted' }, '—')
        : h(ElProgress, {
            percentage: row.successRate,
            color: row.successRate >= 99 ? CHART_SUCCESS : row.successRate >= 95 ? CHART_WARNING : CHART_DANGER,
            strokeWidth: 8,
          }),
  },
  { key: 'avgLatency', title: '平均延迟', width: 110, align: 'right', cell: (row) => fmtMs(row.avgLatency) },
  { key: 'dataPoints', title: '数据点', width: 110, align: 'right' },
]

const { hidden: hiddenColumns } = useColumnPrefs('stats-report')

const busColumns: ColumnDef<BusStatView>[] = [
  { key: 'bus', title: '总线', minWidth: 180 },
  { key: 'protocol', title: '协议', width: 110 },
  { key: 'deviceCount', title: '设备数', width: 90, align: 'center' },
  {
    key: 'onlineDevices',
    title: '在线设备',
    width: 100,
    align: 'center',
    cell: (row) =>
      h('span', { class: row.onlineDevices < row.deviceCount ? 'text-danger' : undefined }, String(row.onlineDevices)),
  },
  { key: 'totalReads', title: '总采集', width: 120, align: 'right' },
  {
    key: 'successRate',
    title: '成功率',
    width: 140,
    cell: (row) =>
      row.successRate == null
        ? h('span', { class: 'text-muted' }, '—')
        : h(ElProgress, { percentage: row.successRate, strokeWidth: 8 }),
  },
  {
    key: 'timeoutRate',
    title: '超时率',
    width: 110,
    align: 'right',
    cell: (row) =>
      h('span', { class: row.timeoutRate != null && row.timeoutRate > 0 ? 'text-danger' : undefined }, fmtPct(row.timeoutRate)),
  },
  {
    key: 'lastErrorCode',
    title: '最近错误码',
    width: 130,
    cell: (row) =>
      row.lastErrorCode
        ? h(ElTag, { size: 'small', type: 'danger', effect: 'plain' }, () => row.lastErrorCode as string)
        : h('span', { class: 'text-muted' }, '—'),
  },
  { key: 'reconnectCount', title: '重连次数', width: 100, align: 'right' },
  { key: 'avgLatency', title: '平均延迟', width: 110, align: 'right', cell: (row) => fmtMs(row.avgLatency) },
]

// 报表取数统一走 useStatsReport（单一数据入口）
const { report, period, load } = useStatsReport()

// 后端对「窗口内无采集」返回 null，统一渲染为「—」，不用 0 冒充
const fmtPct = (v: number | null) => (v == null ? '—' : `${v}%`)
const fmtMs = (v: number | null) => (v == null ? '—' : `${v} ms`)

function exportCsv() {
  if (!report.value) return
  exportReportCsv(report.value)
  ElMessage.success('报表已导出')
}

onMounted(load)
</script>

<style scoped>
.report { padding: 0px; }
.top-bar { display: flex; gap: 8px; margin-bottom: 16px; align-items: center; }
.summary-cards { display: grid; grid-template-columns: repeat(4, 1fr); gap: 12px; }
.section-card { border-radius: var(--radius-card); }
.text-danger { color: var(--text-danger); font-weight: 600; }
.text-muted { color: var(--text-secondary); }
.error-empty { color: var(--text-secondary); font-size: 0.8125rem; }
.error-hint { color: var(--text-secondary); font-size: 0.75rem; margin-bottom: 8px; }
.error-dist { display: flex; flex-direction: column; gap: 10px; }
.error-bar { display: flex; align-items: center; gap: 12px; }
.error-name { width: 120px; font-size: 0.8125rem; flex-shrink: 0; }
.error-track { flex: 1; height: 16px; background: var(--bg-fill); border-radius: var(--radius-table); overflow: hidden; }
.error-fill { height: 100%; background: linear-gradient(90deg, var(--color-warning), var(--color-danger)); border-radius: var(--radius-table); transition: width .3s; }
.error-count { width: 120px; font-size: 0.75rem; color: var(--text-secondary); text-align: right; flex-shrink: 0; }
</style>
