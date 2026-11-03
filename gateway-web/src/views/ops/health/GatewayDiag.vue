<template>
  <div class="gd"><SectionHeader title="自诊断" icon="Cpu" />
    <!-- R2：网关级健康结论已收敛到「运维 › 健康报告」唯一结论页。
         本页只保留一行摘要 + 深链，不再自渲染一张结论卡（与相邻的健康报告页签会给出两个结论）；
         组件清单与问题清单仍复用同一份 /api/health/report 快照；下方 CPU/网络/串口明细走自诊断接口。 -->
    <HealthConclusionBar
      :problems="report?.problems ?? []"
      :status="overallStatus"
      :generated-at-ms="report?.generated_at_ms"
      :summary-text="summaryText"
    />
    <HealthStatus :health-rows="healthRows" :loading="loading" />
    <HealthProblemList :problems="report?.problems ?? []" />
    <el-row :gutter="16">
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">CPU 温度</div><div class="v" :class="cpuTempWarn ? 'warn' : ''">{{ fmtTemp(diag.cpuTemp) }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">CPU 使用率</div><div class="v">{{ fmtPct(diag.cpuUsage) }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">内存使用率</div><div class="v">{{ fmtPct(diag.memUsage) }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">磁盘使用率</div><div class="v">{{ fmtPct(diag.diskUsage) }}</div></el-card></el-col>
    </el-row>
    <el-row :gutter="16" style="margin-top:12px">
      <el-col :span="12">
        <el-card shadow="never" class="section-card"><template #header><strong>系统信息</strong></template>
          <!-- P3-5：区分「主机开机时长」与「gatewayd 进程运行时长」，
               避免重启后观察时把主机时长误当进程时长 -->
          <div class="info"><span>进程运行时长</span><b>{{ diag.processUptime }}</b></div>
          <div class="info"><span>主机开机时长</span><b>{{ diag.hostUptime }}</b></div>
          <div class="info"><span>负载均值</span><b>{{ fmtNum(diag.loadAvg) }}</b></div>
          <div class="info"><span>进程数</span><b>{{ fmtNum(diag.processes) }}</b></div>
        </el-card>
      </el-col>
      <el-col :span="12">
        <el-card shadow="never" class="section-card"><template #header><strong>网络接口</strong></template>
          <!-- 网络配置页（只读展示同一 /api/diagnostics/gateway 数据）已并入本页：
               接口配置需走系统命令行，前端只展示实时状态与收发统计，避免两处同源页面。 -->
          <p class="net-hint">接口配置（IP/掩码/网关/DNS/DHCP）需通过系统命令行修改，此处仅展示实时运行状态。</p>
          <!-- diag 初始为空对象，兜底空数组避免 DataTable 首次渲染时取不到行数据 -->
          <DataTable :data="diag.netInterfaces || []" :columns="netColumns" row-key="name" max-height="200px" empty-text="暂无接口" />
        </el-card>
      </el-col>
    </el-row>
    <el-card shadow="never" class="section-card"><template #header><strong>串口状态</strong></template>
      <DataTable :data="diag.serialPorts || []" :columns="serialColumns" row-key="name" max-height="180px" empty-text="暂无串口" />
    </el-card>
  </div>
</template>
<script setup lang="ts">
import { ref, onMounted, computed } from 'vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { tagColumn } from '@/utils/commonColumns'
import { getGatewayDiagView as getDiag, type GatewayDiagView as GatewayDiag, type NetInterfaceView, type SerialPortView } from '@/api/diagnostics'
import { useHealthReport } from '@/composables/system/useHealthReport'
import { healthRowsFromComponents } from '@/utils/healthStatus'
import HealthStatus from '@/views/overview/HealthStatus.vue'
import HealthConclusionBar from '@/components/HealthConclusionBar.vue'
import HealthProblemList from './HealthProblemList.vue'

const diag = ref<GatewayDiag>({} as GatewayDiag)

// 健康结论 / 组件清单 / 问题清单来自聚合报告（与总览/健康报告同源）
const { report, loading, refresh, overallStatus } = useHealthReport()
const healthRows = computed(() => healthRowsFromComponents(report.value?.components))
// 一行摘要：总线数与在线设备数，完整结论（逐总线明细/分级处置）在健康报告页
const summaryText = computed(() => {
  const s = report.value?.summary
  if (!s) return ''
  return `${s.bus_count} 条总线 · ${s.online_devices}/${s.total_devices} 台设备在线`
})

// 后端 report 只有 storage 组件的 ok/degraded，没有磁盘余量具体数值；
// 「磁盘使用率」这块仍用本页自诊断接口（/api/diagnostics/gateway）补足，不扩张报告结构。

// 后端对「不支持的平台 / 读取失败」返回 null，统一渲染为「—」，绝不显示 0 冒充真实值
const fmtPct = (v: number | null | undefined) => (v === null || v === undefined ? '—' : `${v}%`)
const fmtTemp = (v: number | null | undefined) => (v === null || v === undefined ? '—' : `${v}℃`)
const fmtNum = (v: number | null | undefined) => (v === null || v === undefined ? '—' : String(v))
const cpuTempWarn = computed(() => (diag.value.cpuTemp ?? 0) > 60)

// 网络接口 / 串口两张小表统一走 DataTable；name 在各自列表内唯一，直接作为行键
const netColumns: ColumnDef<NetInterfaceView>[] = [
  { key: 'name', title: '接口', minWidth: 100 },
  { key: 'ip', title: 'IP', minWidth: 130 },
  tagColumn<NetInterfaceView>('status', {
    title: '状态',
    width: 80,
    type: (row) => (row.status === 'up' ? 'success' : 'info'),
    effect: 'light',
    round: false,
  }),
  // 收发统计原属「网络配置」页，页面合并后在此一并展示（同一数据源）
  { key: 'rx', title: '接收', width: 100, cell: (row) => formatBytes(row.rx) },
  { key: 'tx', title: '发送', width: 100, cell: (row) => formatBytes(row.tx) },
]

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`
}

const serialColumns: ColumnDef<SerialPortView>[] = [
  { key: 'name', title: '串口' },
  { key: 'desc', title: '说明' },
  tagColumn<SerialPortView>('status', {
    title: '状态',
    width: 80,
    type: (row) => (row.status === 'ok' ? 'success' : 'danger'),
    effect: 'light',
    round: false,
  }),
]

onMounted(async () => {
  // 健康报告与其它页面共享快照（TTL 10s 内复用，不额外打请求）
  refresh()
  // 数据源：/api/diagnostics/gateway 的真实采集（Linux 读 /proc、/sys，Windows 走系统 API）；
  // 取不到的字段由后端返回 null，页面显示「—」，不做本地估算或 mock 兜底。
  diag.value = await getDiag()
})
</script>
<style scoped>
.gd{padding:16px}.v.warn{color: var(--text-warning)}.section-card{margin-bottom:16px}.info{display:flex;justify-content:space-between;padding:8px 0;border-bottom:1px solid var(--el-border-color-lighter)}
.net-hint{margin:0 0 8px;font-size:0.75rem;color:var(--text-secondary);line-height:1.5}
/* S9: 1366×768 笔记本上本页纵向滚动达 647px。这里只压缩不重排：
 * 指标卡内边距/字号收紧，系统信息行间距减半，让首屏能看到「指标 + 系统信息 + 网络接口」。 */
.gd :deep(.mcard .el-card__body) { padding: 10px 12px; }
.gd :deep(.mcard .v) { font-size: 1.25rem; margin-top: 2px; }
.gd .info { padding: 4px 0; }
</style>
