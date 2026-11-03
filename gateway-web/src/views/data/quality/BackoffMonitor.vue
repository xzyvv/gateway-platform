<template>
  <div class="bm">
    <SectionHeader title="采集退避" icon="Odometer">
      <template #extra>
        <el-tag :type="backingOffCount > 0 ? 'warning' : 'success'" effect="dark">
          退避中 {{ backingOffCount }} / {{ totalCount }} 点位
        </el-tag>
      </template>
      <el-button size="small" :loading="loading" @click="load">
        <el-icon><Refresh /></el-icon> 刷新
      </el-button>
      <el-switch v-model="autoRefresh" active-text="5秒轮询" size="small" style="margin-left:8px" />
      <TablePrefsMenu table-id="backoff-monitor" :columns="pointColumns.map(c => ({ key: c.key, title: c.title }))" />
    </SectionHeader>

    <el-alert type="info" :closable="false" class="tip">
      采集退避：点位连续采集失败 ≥3 次后，网关自动拉长该点位的采集间隔（×5 → ×15 → ×60），
      避免故障设备拖慢整条总线；设备恢复后间隔自动还原，期间上送最后一次成功的值（质量码标记为 Uncertain）。
      看到退避通常意味着：设备断电 / 断线、地址配置错误或总线干扰。
    </el-alert>

    <el-row :gutter="16">
      <el-col :span="8">
        <el-card shadow="never" class="stat">
          <div class="lbl">正常采集</div>
          <div class="val good">{{ normalCount }}</div>
        </el-card>
      </el-col>
      <el-col :span="8">
        <el-card shadow="never" class="stat">
          <div class="lbl">失败中（未退避）</div>
          <div class="val uncertain">{{ failingCount }}</div>
        </el-card>
      </el-col>
      <el-col :span="8">
        <el-card shadow="never" class="stat">
          <div class="lbl">退避中</div>
          <div class="val bad">{{ backingOffCount }}</div>
        </el-card>
      </el-col>
    </el-row>

    <el-card shadow="never" class="section-card">
      <template #header><strong>退避详情</strong></template>
      <DataTable :data="points" :columns="pointColumns" :hidden-columns="hiddenColumns" row-key="point_id" stripe :max-height="520" empty-text="暂无退避中的点位" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, watch, h } from 'vue'
import { usePolling } from '@/composables/core/usePolling'
import { ElTag } from 'element-plus'
import { Refresh } from '@element-plus/icons-vue'
import { QUALITY_LABEL, QUALITY_COLOR, type PointBackoff, type Quality } from '@/api/dataQuality'
import { usePointBackoff } from '@/composables/data/usePointBackoff'
import type { ColumnDef } from '@/components/DataTable.vue'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
import { formatDateTime } from '@/utils/datetime'

// 三类计数统一由 usePointBackoff 的 stats 提供，页面不再各自 filter
const { points, loading, stats, load } = usePointBackoff()
const autoRefresh = ref(false)

const totalCount = computed(() => stats.value.total)
const backingOffCount = computed(() => stats.value.backingOff)
const normalCount = computed(() => stats.value.normal)
const failingCount = computed(() => stats.value.failing)

function qLabel(q: string): string { return QUALITY_LABEL[q as Quality] ?? String(q) }
function qColor(q: string): string { return QUALITY_COLOR[q as Quality] ?? '' }
function multiplierTag(m: number): string {
  if (m >= 60) return 'danger'
  if (m >= 15) return 'warning'
  if (m >= 5) return 'warning'
  return 'info'
}

/** multiplierTag / qColor 返回 string，h() 渲染 el-tag 需收窄为具体标签色 */
type TagType = 'primary' | 'success' | 'info' | 'warning' | 'danger'

// 表格统一走 DataTable；点位级行数可能较多，超阈值自动切虚拟滚动
const pointColumns: ColumnDef<PointBackoff>[] = [
  { key: 'point_name', title: '点位', minWidth: 120 },
  { key: 'device_name', title: '设备', minWidth: 120 },
  { key: 'bus', title: '总线', width: 100 },
  { key: 'base_interval_ms', title: '基础间隔', width: 90, cell: (row) => `${row.base_interval_ms} ms` },
  {
    key: 'current_interval_ms',
    title: '当前间隔',
    width: 90,
    cell: (row) =>
      h(
        ElTag,
        { size: 'small', type: row.current_interval_ms > row.base_interval_ms ? 'warning' : 'info' },
        () => `${row.current_interval_ms} ms`,
      ),
  },
  {
    key: 'backoff_multiplier',
    title: '退避倍率',
    width: 90,
    cell: (row) => h(ElTag, { size: 'small', type: multiplierTag(row.backoff_multiplier) as TagType }, () => `${row.backoff_multiplier}x`),
  },
  { key: 'consecutive_failures', title: '连续失败', width: 90 },
  { key: 'stale_count', title: '重发次数', width: 90 },
  {
    key: 'quality',
    title: '质量',
    width: 100,
    cell: (row) => h(ElTag, { size: 'small', type: qColor(row.quality) as TagType }, () => qLabel(row.quality)),
  },
  {
    key: 'last_good_value',
    title: '上次成功值',
    width: 120,
    cell: (row) => (row.last_good_value === null ? '—' : Number(row.last_good_value).toFixed(3)),
  },
  {
    key: 'last_good_ts',
    title: '上次成功时间',
    width: 150,
    cell: (row) => (row.last_good_ts > 0 ? formatDateTime(row.last_good_ts) : '—'),
  },
]

const { hidden: hiddenColumns } = useColumnPrefs('backoff-monitor')

onMounted(() => {
  load()
})

// 自动刷新轮询：usePolling 统一提供隐藏暂停 / 失败退避 / 卸载清理
const { start: startPoll, stop: stopPoll } = usePolling(load, 5000, { autoStart: false })

watch(autoRefresh, (on) => {
  if (on) startPoll()
  else stopPoll()
})
</script>

<style scoped>
.bm { padding: 16px; }
.tip { margin-bottom: 16px; }
.stat { margin-bottom: 16px; text-align: center; }
.stat .lbl { font-size: 0.8125rem; color: var(--el-text-color-secondary); }
.stat .val { font-size: 1.625rem; font-weight: 600; margin-top: 6px; }
.stat .val.good { color: var(--text-success); }
.stat .val.bad { color: var(--text-danger); }
.stat .val.uncertain { color: var(--text-warning); }
.section-card { margin-bottom: 16px; }
</style>
