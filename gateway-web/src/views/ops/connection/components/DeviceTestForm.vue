<template>
  <el-card shadow="never" class="ct-section">
    <template #header>
      <div class="ct-section-header">
        <span><el-icon><Cpu /></el-icon> 设备采集链路（{{ devices.length }}）</span>
        <el-button size="small" @click="emit('test-all')" :loading="testing" :disabled="devices.length === 0">
          测试全部设备
        </el-button>
      </div>
    </template>

    <DataTable
      :data="devices"
      :columns="columns"
      row-key="id"
      empty-text="暂无设备"
    />
  </el-card>
</template>

<script setup lang="ts">
/**
 * DeviceTestForm — 设备采集链路测试表单（展示型组件）
 *
 * 由 ConnectionTest 容器传入 devices（含 _status/_latency/_error 测试状态字段），
 * 本组件只负责渲染表格与状态标签，所有测试逻辑通过 emit 上抛容器处理：
 *   - test-one: 单行"测试"按钮点击，payload = DeviceRow
 *   - test-all: "测试全部设备"按钮点击
 */
import { h } from 'vue'
import { ElButton, ElTag } from 'element-plus'
import { Cpu } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import type { DeviceRow } from './types'

defineProps<{
  devices: DeviceRow[]
  testing: boolean
}>()

const emit = defineEmits<{
  'test-one': [dev: DeviceRow]
  'test-all': []
}>()

/** 测试状态标签：与迁移前的 v-if 分支一一对应 */
function statusTag(status: DeviceRow['_status']) {
  if (status === 'ok') return h(ElTag, { type: 'success', size: 'small' }, () => '在线')
  if (status === 'fail') return h(ElTag, { type: 'danger', size: 'small' }, () => '离线')
  if (status === 'testing') return h(ElTag, { type: 'warning', size: 'small' }, () => '测试中')
  return h(ElTag, { type: 'info', size: 'small' }, () => '未测试')
}

/** 详情列：错误信息 > 读取成功 > 占位符 */
function detailCell(row: DeviceRow) {
  if (row._error) return h('span', { class: 'ct-error' }, row._error)
  if (row._status === 'ok') return h('span', { class: 'ct-ok' }, '读取成功')
  return h('span', { class: 'ct-muted' }, '-')
}

// 表格统一走 DataTable；列顺序/宽度与迁移前保持一致
const columns: ColumnDef<DeviceRow>[] = [
  { key: 'name', title: '设备名称', minWidth: 140 },
  { key: 'protocol', title: '协议', width: 100 },
  { key: 'bus', title: '总线', width: 100 },
  { key: 'address', title: '地址/从站号', width: 120, cell: (row) => row.address || row.slave_id || '-' },
  { key: 'status', title: '状态', width: 100, cell: (row) => statusTag(row._status) },
  { key: 'latency', title: '耗时', width: 100, cell: (row) => (row._latency != null ? `${row._latency}ms` : '-') },
  { key: 'detail', title: '详情', minWidth: 160, cell: (row) => detailCell(row) },
  actionColumn<DeviceRow>({
    width: 100,
    fixed: 'right',
    cell: (row) =>
      h(ElButton, { size: 'small', type: 'primary', link: true, loading: row._status === 'testing', onClick: () => emit('test-one', row) }, () => '测试'),
  }),
]
</script>

<style scoped>
.ct-section { margin-bottom: 16px; }
.ct-section-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  font-weight: 600;
}
.ct-ok { color: var(--text-success); }
.ct-error { color: var(--text-danger); font-size: 0.75rem; }
.ct-muted { color: var(--text-placeholder); }
</style>