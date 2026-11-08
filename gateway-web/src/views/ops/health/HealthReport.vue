<template>
  <div class="hr">
    <SectionHeader title="健康报告">
      <template #extra>
        <el-tag :type="statusMeta.type" effect="dark" size="small">{{ statusMeta.label }}</el-tag>
      </template>
      <el-button size="small" :loading="loading" @click="refresh(true)">刷新</el-button>
    </SectionHeader>

    <el-row :gutter="16" class="cards">
      <el-col :span="6">
        <el-card shadow="never" class="mcard">
          <div class="l">设备在线</div>
          <div class="v" :class="summary.online_devices === 0 && summary.total_devices > 0 ? 'bad' : ''">
            {{ summary.online_devices }}/{{ summary.total_devices }}
          </div>
        </el-card>
      </el-col>
      <el-col :span="6">
        <el-card shadow="never" class="mcard">
          <div class="l">队列存量</div>
          <div class="v" :class="queueBad ? 'warn' : ''">{{ summary.queue_len }} 条</div>
        </el-card>
      </el-col>
      <el-col :span="6">
        <el-card shadow="never" class="mcard">
          <div class="l">近 24h 成功率</div>
          <div class="v">{{ fmtPct(summary.success_rate) }}</div>
        </el-card>
      </el-col>
      <el-col :span="6">
        <el-card shadow="never" class="mcard">
          <div class="l">总线 / 采集次数</div>
          <div class="v">{{ summary.bus_count }} / {{ summary.total_reads }}</div>
        </el-card>
      </el-col>
    </el-row>

    <!-- 磁盘水位：分级告警的可见落点。剩余空间掉到 25%/15%/10% 会依次进入
         预警/严重/阻断档；此处把档位与实际余量摆在一行，让现场不用去翻曲线图
         就能判断"还有多少天要动手"。 -->
    <el-card shadow="never" class="section-card">
      <template #header><strong>磁盘水位</strong></template>
      <div class="disk-row">
        <el-tag :type="diskTagType" effect="dark" size="small">{{ diskLevelLabel }}</el-tag>
        <span class="disk-pct">{{ summary.disk_free_pct === null ? '—' : `${summary.disk_free_pct.toFixed(1)}%` }}</span>
        <span class="disk-hint">
          剩余空间；预警 {{ summary.disk_thresholds.warn }}% / 严重 {{ summary.disk_thresholds.critical }}% / 阻断 {{ summary.disk_thresholds.blocking }}%
        </span>
      </div>
    </el-card>

    <el-card shadow="never" class="section-card">
      <template #header><strong>组件状态</strong></template>
      <div class="chips">
        <el-tag v-for="(v, k) in components" :key="k" :type="componentType(v)" size="small">
          {{ k }}：{{ v }}
        </el-tag>
        <span class="generated">{{ generatedAt }}</span>
      </div>
    </el-card>

    <!-- B1：问题清单改为共享组件，与自诊断 / 网关信息页同一份分级与文案 -->
    <HealthProblemList :problems="report?.problems ?? []" />

    <el-card shadow="never" class="section-card">
      <template #header><strong>总线明细（近 24h）</strong></template>
      <DataTable :data="buses" :columns="busColumns" row-key="bus" max-height="280px" empty-text="暂无总线" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * 健康报告页签（P1-3）
 *
 * 后端 `/api/health/report` 早就实现了「组件状态 + 汇总 + 逐总线 + 分级问题清单」，
 * 但前端一直没有入口，现场只能自己对比 `/api/status`、`/api/watchdog/services`、
 * `/api/north/status` 三份 JSON 才能发现「队列背压」。这里把它接成一个页签，
 * problems 按 critical → warning → info 分级列出，处置提示（hint）直接展示。
 */
import { computed, h, onMounted } from 'vue'
import { ElTag } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import type { HealthBusRow } from '@/api/status'
import { useHealthReport } from '@/composables/system/useHealthReport'
import { busSuccessTag, QUEUE_BACKLOG_WARN } from '@/utils/healthVerdict'
import HealthProblemList from './HealthProblemList.vue'

// 与运行总览 / 自诊断 / 网关信息共享同一份模块级快照；手动刷新强制绕过 10s TTL
const { report, loading, refresh, overallStatus } = useHealthReport()

const summary = computed(
  () =>
    report.value?.summary ?? {
      total_devices: 0,
      collecting_devices: 0,
      online_devices: 0,
      bus_count: 0,
      queue_len: 0,
      total_reads: 0,
      success_rate: null as number | null,
      disk_level: 'ok' as const,
      disk_free_pct: null as number | null,
      disk_thresholds: { warn: 25, critical: 15, blocking: 10 },
    },
)
const components = computed(() => report.value?.components ?? {})
const buses = computed(() => report.value?.buses ?? [])

const statusMeta = computed(() => {
  // 首次加载时沿用「读取中」，避免把「还没数据」显示成「暂无数据」
  if (overallStatus.value.value === 'unknown' && loading.value) {
    return { label: '读取中', type: 'info' as const }
  }
  return overallStatus.value
})

/** 队列存量 ≥ 预警阈值时标黄；具体分级以后端 problems 为准，这里只做视觉提示 */
const queueBad = computed(() => summary.value.queue_len >= QUEUE_BACKLOG_WARN)

const componentType = (v: string) =>
  v === 'ok' ? ('success' as const) : v === 'down' ? ('danger' as const) : ('warning' as const)

/** 磁盘水位档位文案（与后端 DiskLevel 同名同义） */
const DISK_LEVEL_LABEL: Record<string, string> = {
  ok: '正常',
  warn: '预警',
  critical: '严重',
  blocking: '阻断',
}

const diskLevelLabel = computed(
  () => DISK_LEVEL_LABEL[summary.value.disk_level] ?? '正常',
)

const diskTagType = computed(() => {
  switch (summary.value.disk_level) {
    case 'blocking':
      return 'danger' as const
    case 'critical':
      return 'danger' as const
    case 'warn':
      return 'warning' as const
    default:
      return 'success' as const
  }
})

const fmtPct = (v: number | null | undefined) => (v === null || v === undefined ? '—' : `${v}%`)

const generatedAt = computed(() => {
  const ms = report.value?.generated_at_ms
  if (!ms) return ''
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `生成于 ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
})

const busColumns: ColumnDef<HealthBusRow>[] = [
  { key: 'bus', title: '总线', minWidth: 160, mono: true },
  { key: 'protocol', title: '协议', width: 90 },
  {
    key: 'online_devices',
    title: '在线/设备',
    width: 100,
    cell: (row) =>
      h(
        'span',
        { class: row.online_devices === 0 && row.total_devices > 0 ? 'bad' : '' },
        `${row.online_devices}/${row.total_devices}`,
      ),
  },
  { key: 'total_reads', title: '采集次数', width: 100, align: 'right', mono: true },
  {
    key: 'success_rate',
    title: '成功率',
    width: 100,
    align: 'right',
    cell: (row) => {
      const v = row.success_rate
      if (v === null || v === undefined) return '—'
      // 标色阈值与交付自检共用 healthVerdict，避免两页对同一总线给出不同颜色/结论
      return h(ElTag, { size: 'small', type: busSuccessTag(v) }, () => `${v}%`)
    },
  },
  { key: 'avg_latency_ms', title: '平均延迟', width: 100, align: 'right', cell: (row) => (row.avg_latency_ms === null ? '—' : `${row.avg_latency_ms}ms`) },
  { key: 'timeout_rate', title: '超时率', width: 90, align: 'right', cell: (row) => fmtPct(row.timeout_rate) },
  { key: 'last_error_code', title: '最近错误', width: 110, cell: (row) => row.last_error_code ?? '—' },
  { key: 'reconnect_count', title: '重连次数', width: 100, align: 'right', mono: true },
]

onMounted(() => {
  // 首次进入复用模块级快照（10s 内其它页面已拉过就直接显示，不再重复请求）
  refresh()
})
</script>

<style scoped>
.hr { padding: 16px; }
.cards { margin: 12px 0 16px; }
.mcard .l { color: var(--text-secondary); font-size: 0.8rem; }
.mcard .v { font-size: 1.35rem; font-weight: 600; margin-top: 2px; }
.mcard .v.bad { color: var(--text-danger); }
.mcard .v.warn { color: var(--text-warning); }
.section-card { margin-bottom: 16px; }
.chips { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
.generated { margin-left: auto; color: var(--text-placeholder); font-size: 0.78rem; }
.bad { color: var(--text-danger); }
.disk-row { display: flex; align-items: center; gap: 12px; flex-wrap: wrap; }
.disk-pct { font-size: 1.35rem; font-weight: 600; }
.disk-hint { color: var(--text-secondary); font-size: 0.8rem; }
</style>
