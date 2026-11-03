<template>
  <el-card class="batch-card" shadow="never">
    <SectionHeader title="批量写入（多点位）" icon="Upload">
      <!-- 操作按钮放在区块标题栏：卡片底部在 1366/1280 视口会被挤出首屏，
           提交按钮留在底部时用户填完表还得滚回去才能提交 -->
      <el-button size="small" @click="addBatchRow">
        <el-icon><Plus /></el-icon> 新增一行
      </el-button>
      <el-button
        size="small"
        type="warning"
        :loading="batchLoading"
        :disabled="!deviceId"
        @click="doBatchWrite"
      >
        <el-icon><Check /></el-icon> 批量写入
      </el-button>
    </SectionHeader>
    <p class="batch-tip">
      一次提交多个点位的目标值：同一总线的点位会自动合并为一次事务下发（如 Modbus 多点写），减少通讯往返；
      单点失败不影响其余点位。点位列表来自上方所选设备。
    </p>

    <DataTable
      :data="batchRows"
      :columns="batchColumns"
      :row-key="batchRowKey"
      class="batch-table"
    />

    <div v-if="batchSummary" class="batch-result">
      <el-tag size="small" :type="batchSummaryType" effect="dark">{{ batchSummary }}</el-tag>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 批量写入（多点位）卡片（从 Tester.vue 拆出）
 *
 * 职责：维护批量写入表格的行数据与列定义，收集多点位目标值后一次性下发，
 * 并把执行结果按行回填；执行结论通过 log 事件交回父组件写入「操作记录」。
 *
 * Props:
 * - points：当前设备的点位列表（表格里的点位下拉数据源）
 * - deviceId：当前设备 id；为空时禁止提交，设备切换时清空已填内容
 *
 * Emits:
 * - log：批量写入的成功/失败结论（type / tag / message），由父组件写入操作记录
 *
 * 典型应用场景：点位读写测试页（Tester.vue）底部的批量写入区块（仅协议支持写入时出现）。
 */
import { ref, watch, onUnmounted, h, Fragment } from 'vue'
import { ElMessage, ElButton, ElIcon, ElInputNumber, ElOption, ElSelect, ElTag } from 'element-plus'
import { Check, Delete, Plus } from '@element-plus/icons-vue'
import DataTable from '@/components/DataTable.vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import type { BatchWriteResultItem } from '@/api/points'
import type { PointConfig } from '@/api/types'
import { confirmPointWrite, usePointWrite } from '@/composables/data/usePointWrite'
import { withDiagHint } from '@/utils/diagHints'
import { extractErrorMessage } from '@/utils/errorHandler'
import type { LogEntry } from './OperationLogPanel.vue'

interface BatchRow {
  pointId: string
  value: number | null
  result: BatchWriteResultItem | null
}

function newBatchRow(): BatchRow {
  return { pointId: '', value: null, result: null }
}

const props = defineProps<{
  /** 当前设备的点位列表 */
  points: PointConfig[]
  /** 当前设备 id */
  deviceId: string
}>()

const emit = defineEmits<{
  /** 批量写入结论（写入父组件的操作记录） */
  (e: 'log', type: LogEntry['type'], tag: string, message: string): void
}>()

const { writeBatch } = usePointWrite()

const batchRows = ref<BatchRow[]>([newBatchRow()])
const batchLoading = ref(false)
const batchSummary = ref('')
const batchSummaryType = ref<'success' | 'warning' | 'danger'>('success')

let unmounted = false

/** 批量行无稳定主键，用数组下标做行键（行内编辑，无勾选/分页） */
const batchRowKey = (row: BatchRow) => String(batchRows.value.indexOf(row))

// 批量写入列定义：序号列由 cell 按下标补出，其余列复刻原表格的控件与结果展示
const batchColumns: ColumnDef<BatchRow>[] = [
  {
    key: 'index',
    title: '#',
    width: 50,
    align: 'center',
    cell: (row) => String(batchRows.value.indexOf(row) + 1),
  },
  {
    key: 'pointId',
    title: '点位',
    minWidth: 240,
    cell: (row) =>
      h(
        ElSelect,
        {
          modelValue: row.pointId,
          placeholder: '请选择点位',
          filterable: true,
          clearable: true,
          size: 'small',
          style: 'width: 100%',
          'onUpdate:modelValue': (v: string | undefined) => { row.pointId = v ?? '' },
        },
        () =>
          props.points.map((p) =>
            h(ElOption, {
              key: p.id,
              label: `${p.name || p.id} (${p.data_type || p.protocol || ''})`,
              value: p.id,
            }),
          ),
      ),
  },
  {
    key: 'value',
    title: '目标值',
    width: 180,
    cell: (row) =>
      h(ElInputNumber, {
        modelValue: row.value,
        controls: false,
        size: 'small',
        placeholder: '要写入的值',
        style: 'width: 100%',
        'onUpdate:modelValue': (v: number | null | undefined) => { row.value = v ?? null },
      }),
  },
  {
    key: 'result',
    title: '结果',
    minWidth: 220,
    cell: (row) =>
      h(Fragment, null, [
        row.result
          ? h(
              ElTag,
              { type: row.result.status === 'ok' ? 'success' : 'danger', size: 'small', effect: 'dark' },
              () => (row.result!.status === 'ok' ? `成功 ${row.result!.value}` : '失败'),
            )
          : h('span', { class: 'batch-pending' }, '未提交'),
        row.result?.error ? h('span', { class: 'batch-error' }, row.result.error) : null,
      ]),
  },
  actionColumn<BatchRow>({
    width: 70,
    align: 'center',
    cell: (row) => {
      const label = `移除 ${row.pointId || `第 ${batchRows.value.indexOf(row) + 1} 行`}`
      return h(
        ElButton,
        {
          size: 'small',
          text: true,
          type: 'danger',
          // 纯图标按钮没有可见文案，读屏与悬停都需要一个名称
          'aria-label': label,
          title: label,
          onClick: () => removeBatchRow(batchRows.value.indexOf(row)),
        },
        () => h(ElIcon, null, () => h(Delete)),
      )
    },
  }),
]

// 切换设备后点位列表整体变化，清空批量表格中已选点位与上一轮结果
watch(() => props.deviceId, () => {
  batchRows.value = [newBatchRow()]
  batchSummary.value = ''
})

function addBatchRow() {
  batchRows.value.push(newBatchRow())
}

function removeBatchRow(index: number) {
  batchRows.value.splice(index, 1)
  // 至少保留一行，避免表格空置无从下手
  if (!batchRows.value.length) batchRows.value.push(newBatchRow())
}

async function doBatchWrite() {
  const rows = batchRows.value
  if (!rows.length) return
  if (rows.some((r) => !r.pointId || r.value === null || !Number.isFinite(r.value))) {
    ElMessage.warning('请为每一行选择点位并填写有效的数值')
    return
  }
  // 同一点位在一批中重复下发会导致结果错乱，提前拦截
  const ids = rows.map((r) => r.pointId)
  const dup = ids.find((id, i) => ids.indexOf(id) !== i)
  if (dup) {
    ElMessage.warning(`点位「${dup}」在表格中重复，请合并为一行`)
    return
  }
  const entries = rows.map((r) => ({ id: r.pointId, value: r.value as number }))
  if (!(await confirmPointWrite(entries))) return
  if (unmounted) return

  batchLoading.value = true
  batchSummary.value = ''
  rows.forEach((r) => { r.result = null })
  const t0 = performance.now()
  try {
    // 统一写入口：只传数组，1 条由后端按单写处理，≥2 条按批量写处理
    const outcome = await writeBatch(entries)
    if (unmounted) return
    const latency = performance.now() - t0
    // results 与入参等长且逐位对应，按下标回填各行结果
    rows.forEach((r, i) => { r.result = outcome.results[i] ?? null })
    const summaryType =
      outcome.failed === 0 ? 'success' : outcome.succeeded === 0 ? 'danger' : 'warning'
    if (outcome.error) {
      // 请求级失败（网络/权限）：没有逐条结果，只回填汇总与操作记录
      const msg = withDiagHint(outcome.error)
      batchSummary.value = `批量写入失败：${msg}`
      batchSummaryType.value = 'danger'
      emit('log', 'danger', '批量写入', batchSummary.value)
      ElMessage.error(batchSummary.value)
      return
    }
    batchSummary.value = `成功 ${outcome.succeeded} / 失败 ${outcome.failed}（共 ${outcome.total}，${latency.toFixed(0)}ms）`
    batchSummaryType.value = summaryType
    emit('log', summaryType, '批量写入', batchSummary.value)
  } catch (e) {
    if (unmounted) return
    // 网络/权限等请求级异常没有逐条结果，同样按失败上报，避免「点了没反应」
    const msg = withDiagHint(extractErrorMessage(e, '未知错误'))
    batchSummary.value = `批量写入失败：${msg}`
    batchSummaryType.value = 'danger'
    emit('log', 'danger', '批量写入', batchSummary.value)
    ElMessage.error(batchSummary.value)
  } finally {
    if (!unmounted) batchLoading.value = false
  }
}

onUnmounted(() => { unmounted = true })
</script>

<style scoped>
/* 以下规则来自原 Tester.css，随区块一起迁移到本组件 */
.batch-tip {
  font-size: 0.75rem;
  color: var(--text-secondary);
  margin: 0 0 12px;
  line-height: 1.6;
}
.batch-table {
  margin-bottom: 12px;
}
/* 提交按钮已移到区块标题栏，这里只回填执行结果汇总 */
.batch-result {
  display: flex;
  justify-content: flex-end;
}
.batch-pending {
  font-size: 0.75rem;
  color: var(--text-secondary);
}
.batch-error {
  display: block;
  margin-top: 2px;
  font-size: 0.75rem;
  color: var(--text-danger);
  word-break: break-all;
}
</style>
