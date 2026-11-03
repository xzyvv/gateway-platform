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
const { showToast } = useErrorDisplay()
import type { ColumnDef } from '@/components/DataTable.vue'
import { getConfig } from '@/api/config'
import type { DeviceConfig, PointConfig } from '@/api/types'

interface MergeGroup { startAddr: number; endAddr: number; count: number }
interface DeviceOptimize {
  deviceName: string
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

// 表格统一走 DataTable；列顺序/宽度与迁移前保持一致
const columns: ColumnDef<DeviceOptimize>[] = [
  { key: 'deviceName', title: '设备', minWidth: 140 },
  { key: 'pointCount', title: '点位数', width: 80 },
  { key: 'originalReads', title: '原始读取', width: 90 },
  { key: 'mergedReads', title: '合并读取', width: 90 },
  {
    key: 'reduction',
    title: '优化率',
    width: 120,
    cell: (row) => h(ElProgress, { percentage: row.reduction, status: row.reduction > 0 ? 'success' : 'exception', strokeWidth: 10 }),
  },
  {
    key: 'groups',
    title: '合并组',
    minWidth: 300,
    cell: (row) =>
      h(
        'span',
        null,
        row.groups.map((g) =>
          h(ElTag, { key: `${g.startAddr}-${g.endAddr}`, size: 'small', style: 'margin-right:4px;margin-bottom:2px', effect: 'plain' }, () => `${g.startAddr}-${g.endAddr} (${g.count}个)`),
        ),
      ),
  },
]

const MAX_GAP = 5

function computeMerge(points: PointConfig[]): { groups: MergeGroup[]; merged: number } {
  const addrs = points.map((p) => p.address_num).filter((a): a is number => a != null).sort((a, b) => a - b)
  if (addrs.length === 0) return { groups: [], merged: 0 }
  const groups: MergeGroup[] = []
  let start = addrs[0]!
  let end = addrs[0]!
  let count = 1
  for (let i = 1; i < addrs.length; i++) {
    const cur = addrs[i]!
    if (cur - end <= MAX_GAP) { end = cur; count++ }
    else { groups.push({ startAddr: start, endAddr: end, count }); start = cur; end = cur; count = 1 }
  }
  groups.push({ startAddr: start, endAddr: end, count })
  return { groups, merged: groups.length }
}

async function load() {
  loading.value = true
  try {
    const cfg = await getConfig()
    const devList: DeviceConfig[] = cfg.devices || []
    const allPoints: PointConfig[] = cfg.points || []
    const result: DeviceOptimize[] = []
    let totalPts = 0, totalOrig = 0, totalMerged = 0
    for (const d of devList) {
      const devPoints = allPoints.filter((p) => p.device_id === d.id)
      const pointCount = devPoints.length
      const original = pointCount
      const { groups, merged } = computeMerge(devPoints)
      const reduction = original > 0 ? Math.round((original - merged) / original * 100) : 0
      const savedMs = (original - merged) * 50
      result.push({ deviceName: d.name, pointCount, originalReads: original, mergedReads: merged, reduction, savedMs, groups })
      totalPts += pointCount; totalOrig += original; totalMerged += merged
    }
    devices.value = result
    const avgRed = totalOrig > 0 ? Math.round((totalOrig - totalMerged) / totalOrig * 100) : 0
    summary.value = { totalPoints: totalPts, totalOriginalReads: totalOrig, totalMergedReads: totalMerged, avgReduction: avgRed }
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