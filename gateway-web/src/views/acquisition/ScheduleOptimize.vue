<template>
  <div class="so">
    <SectionHeader title="调度优化" icon="Cpu" />

    <el-row :gutter="16">
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">点位总数</div><div class="val">{{ summary.totalPoints }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">原始读取次数</div><div class="val">{{ summary.totalOriginalReads }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">合并后读取</div><div class="val highlight">{{ summary.totalMergedReads }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="stat"><div class="lbl">平均优化率</div><div class="val highlight">{{ summary.avgReduction }}%</div></el-card></el-col>
    </el-row>

    <el-card shadow="never" class="section-card">
      <template #header><strong>各设备合并详情</strong></template>
      <DataTable
        :data="devices"
        :columns="columns"
        row-key="deviceName"
        stripe
        :loading="loading"
        empty-text="暂无设备，请先在点位管理中为设备配置可采集点位"
      />
    </el-card>

    <el-card shadow="never" class="section-card">
      <template #header><strong>合并原理说明</strong></template>
      <div class="explain">
        <p>同一设备中地址连续的点位会被自动合并为一次 Modbus 读取请求，减少总请求数。</p>
        <p class="formula">原始 N 个点位 = N 次读取；合并后按连续地址 = M 次读取，M ≤ N。</p>
      </div>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h, ref, onMounted } from 'vue'
import { ElProgress, ElTag } from 'element-plus'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import type { ColumnDef } from '@/components/DataTable.vue'
import { getScheduleOptimizeReport } from '@/api/diagnostics/raw'

const { showToast } = useErrorDisplay()

interface MergeGroup { slaveId: number; functionCode: number; startAddr: number; endAddr: number; count: number }
interface DeviceOptimize {
  deviceName: string
  protocol: string
  mergeSupported: boolean
  pointCount: number
  originalReads: number
  mergedReads: number
  reduction: number
  savedMs: number
  groups: MergeGroup[]
}

const devices = ref<DeviceOptimize[]>([])
const summary = ref({ totalPoints: 0, totalOriginalReads: 0, totalMergedReads: 0, avgReduction: 0 })
const loading = ref(false)

// DD-11：合并分组 / 优化率全部由后端 `GET /api/diagnostics/schedule-optimize` 下发，
// 与真实采集路径共用 merge_batches（从站号 / 功能码 / 寄存器宽度 / 自适应 gap）。
// 本页只做展示，不再自行按「地址差 ≤ 5」估算。
const columns: ColumnDef<DeviceOptimize>[] = [
  { key: 'deviceName', title: '设备', minWidth: 140 },
  { key: 'protocol', title: '协议', width: 90 },
  { key: 'pointCount', title: '点位数', width: 80 },
  { key: 'originalReads', title: '原始读取', width: 90 },
  { key: 'mergedReads', title: '合并读取', width: 90 },
  {
    key: 'reduction',
    title: '优化率',
    width: 130,
    cell: (row) =>
      row.mergeSupported
        ? h(ElProgress, { percentage: row.reduction, status: row.reduction > 0 ? 'success' : 'exception', strokeWidth: 10 })
        : h(ElTag, { size: 'small', type: 'info', effect: 'plain' }, () => '不支持合并'),
  },
  {
    key: 'groups',
    title: '合并组（从站/功能码: 起-止）',
    minWidth: 320,
    cell: (row) =>
      h(
        'span',
        null,
        row.groups.map((g) =>
          h(ElTag, { key: `${g.slaveId}-${g.functionCode}-${g.startAddr}-${g.endAddr}`, size: 'small', style: 'margin-right:4px;margin-bottom:2px', effect: 'plain' }, () => `${g.slaveId}/${g.functionCode}: ${g.startAddr}-${g.endAddr} (${g.count}个)`),
        ),
      ),
  },
]

async function load() {
  loading.value = true
  try {
    const r = await getScheduleOptimizeReport()
    devices.value = r.devices.map((d) => ({
      deviceName: d.device_name,
      protocol: d.protocol,
      mergeSupported: d.merge_supported,
      pointCount: d.point_count,
      originalReads: d.original_reads,
      mergedReads: d.merged_reads,
      reduction: Math.round(d.reduction_percent),
      savedMs: Number(d.saved_estimate_ms),
      groups: d.groups.map((g) => ({
        slaveId: g.slave_id,
        functionCode: g.function_code,
        startAddr: g.start_addr,
        endAddr: g.end_addr,
        count: g.point_count,
      })),
    }))
    summary.value = {
      totalPoints: r.total_points,
      totalOriginalReads: r.total_original_reads,
      totalMergedReads: r.total_merged_reads,
      avgReduction: Math.round(r.reduction_percent),
    }
  } catch (e) { showToast(e, '加载调度优化失败') } finally { loading.value = false }
}
onMounted(load)
</script>

<style scoped>
.so { padding: 0px; }
.stat { margin-bottom: 16px; text-align: center; }
.stat .lbl { color: var(--el-text-color-secondary); font-size: 0.8125rem; }
.stat .val { font-size: 1.5rem; font-weight: 600; margin-top: 6px; }
.stat .val.highlight { color: var(--text-success); }
.section-card { margin-bottom: 16px; }
.saved { color: var(--text-success); font-weight: 600; }
.explain { color: var(--el-text-color-regular); line-height: 1.8; }
.formula { background: var(--el-fill-color-light); padding: 10px; border-radius: var(--radius-button); font-family: monospace; }
</style>