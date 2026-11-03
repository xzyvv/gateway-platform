<template>
  <div class="al">
    <el-card shadow="never" class="section-card">
      <template #header>
        <div class="card-header">
          <div class="card-title">
            <strong>变更记录</strong>
            <el-tag size="small" type="info" effect="plain" round>共 {{ total }} 条</el-tag>
          </div>
          <div class="filters">
            <FilterToolbar
              :model-value="filter"
              :fields="filterFields"
              @update:model-value="assignFilter"
              @search="load"
              @reset="reset"
            >
              <el-date-picker
                v-model="timeRange"
                type="datetimerange"
                size="small"
                range-separator="至"
                start-placeholder="开始时间"
                end-placeholder="结束时间"
                format="YYYY-MM-DD HH:mm:ss"
                value-format="x"
                style="width: 360px"
                @change="onTimeChange"
              />
            </FilterToolbar>
          </div>
        </div>
      </template>

      <DataTable
        :data="pageData"
        :columns="logColumns"
        row-key="ts"
        :loading="loading"
        stripe
        empty-text="暂无审计记录"
        v-model:page="page"
        v-model:page-size="pageSize"
        :total="total"
        :page-sizes="pageSizes"
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, watch, onMounted, h } from 'vue'
import { ElTag } from 'element-plus'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { listAuditLogs, type AuditLogEntry, type AuditFilter } from '@/api/auditLog'
import type { ColumnDef } from '@/components/DataTable.vue'
import FilterToolbar, { type FilterField } from '@/components/FilterToolbar.vue'
import { useQueryState } from '@/composables/core/useQueryState'
import { useTablePager } from '@/composables/core/useTablePager'
import { logError } from '@/utils/logger'
import { timeColumn } from '@/utils/commonColumns'
import { formatDateTime } from '@/utils/datetime'

const logs = ref<AuditLogEntry[]>([])
const loading = ref(false)
const filter = reactive<AuditFilter>({
  action: undefined,
  target: undefined,
  source: undefined,
  ts_from: undefined,
  ts_to: undefined,
  limit: 200,
})
const timeRange = ref<[string, string] | null>(null)

// 筛选条件与 URL 双向同步：切 tab 返回后还原，且可刷新/分享带筛选的链接
// （limit 在 query 中为字符串，还原时转数字；时间范围为自定义控件不持久化）
const qsAction = useQueryState('action')
const qsTarget = useQueryState('target')
const qsSource = useQueryState('source')
const qsLimit = useQueryState('limit')
if (qsAction.value) filter.action = qsAction.value
if (qsTarget.value) filter.target = qsTarget.value
if (qsSource.value) filter.source = qsSource.value
if (qsLimit.value) filter.limit = Number(qsLimit.value) || 200
watch(
  () => [filter.action, filter.target, filter.source, filter.limit] as const,
  ([a, t, s, l]) => {
    qsAction.value = a ?? ''
    qsTarget.value = t ?? ''
    qsSource.value = s ?? ''
    qsLimit.value = l ? String(l) : ''
  },
)

// 客户端分页（后端无 limit 截断）；页码/每页行数由 DataTable 页脚接管。
// 走 useTablePager 而非手写切片：每页行数偏好与点位/设备列表共享并持久化，
// 越界夹页逻辑也不再各写一遍。
const {
  page, pageSize, pageSizes, total, paged: pageData, reset: resetPage,
} = useTablePager(logs, () => true, { pageSize: 20, syncUrl: true })

// 审计日志表统一走 DataTable：列顺序/宽度与迁移前保持一致
const logColumns: ColumnDef<AuditLogEntry>[] = [
  timeColumn<AuditLogEntry>('ts', { width: 180, format: (value) => (value ? formatDateTime(value as number) : '-') }),
  { key: 'source', title: '操作来源', width: 120, ellipsis: true },
  {
    key: 'action',
    title: '操作',
    width: 160,
    cell: (row) => h(ElTag, { size: 'small', effect: 'dark', round: true, type: actionType(row.action) }, () => row.action),
  },
  { key: 'target', title: '对象', minWidth: 180, ellipsis: true },
  { key: 'detail', title: '详情', minWidth: 240, ellipsis: true },
]

// 常用 action 前缀（仅作下拉快捷选项，输入也可自定义）
const ACTIONS = [
  'config.update',
  'config.rollback',
  'ota.upload',
  'ota.rollback',
  'device.create',
  'device.update',
  'device.delete',
  'point.create',
  'point.update',
  'point.delete',
]

// 筛选工具栏字段声明（action/limit 下拉 + target/source 输入；时间范围为自定义控件）
const filterFields: FilterField[] = [
  { key: 'action', type: 'select', placeholder: '操作类型', width: 160, options: ACTIONS },
  { key: 'target', type: 'input', placeholder: '对象关键词', width: 160 },
  { key: 'source', type: 'input', placeholder: '操作者来源', width: 140 },
  { key: 'limit', type: 'select', placeholder: '条数', width: 100, options: [100, 200, 500, 1000], clearable: false },
]

// filter 是 reactive 对象，FilterToolbar 的 v-model 输出整体替换值，用 Object.assign 合并回去
function assignFilter(v: Record<string, unknown>) {
  Object.assign(filter, v)
}

function actionType(a: string): 'primary' | 'success' | 'warning' | 'danger' | 'info' {
  if (a.includes('delete') || a.includes('rollback')) return 'danger'
  if (a.includes('create')) return 'success'
  if (a.includes('update')) return 'warning'
  if (a.includes('ota')) return 'info'
  return 'primary'
}

function onTimeChange(val: [string, string] | null) {
  if (val && val.length === 2) {
    filter.ts_from = Number(val[0])
    filter.ts_to = Number(val[1])
  } else {
    filter.ts_from = undefined
    filter.ts_to = undefined
  }
  load()
}

function reset() {
  filter.action = undefined
  filter.target = undefined
  filter.source = undefined
  filter.ts_from = undefined
  filter.ts_to = undefined
  filter.limit = 200
  timeRange.value = null
  resetPage()
  load()
}

async function load() {
  loading.value = true
  try {
    logs.value = await listAuditLogs({ ...filter })
    resetPage()
  } catch (e) {
    showToast(e, '加载审计日志失败')
    logError('audit-log', e)
  } finally {
    loading.value = false
  }
}

onMounted(load)
</script>

<style scoped>
.al { padding: 16px; }
.section-card { margin-bottom: 16px; }
.card-header { display: flex; justify-content: space-between; align-items: center; flex-wrap: wrap; gap: 8px; }
.card-title { display: flex; align-items: center; gap: 8px; }
.filters { display: flex; gap: 8px; flex-wrap: wrap; align-items: center; }
</style>
