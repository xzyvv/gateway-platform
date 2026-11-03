<template>
  <el-card shadow="never" class="ct-section">
    <template #header>
      <div class="ct-section-header">
        <span><el-icon><Promotion /></el-icon> 上行通道（{{ channels.length }}）</span>
        <el-button size="small" @click="emit('test-all')" :loading="testing" :disabled="channels.length === 0">
          测试全部通道
        </el-button>
      </div>
    </template>

    <DataTable
      :data="channels"
      :columns="columns"
      row-key="name"
      empty-text="暂无上行通道"
    />
  </el-card>
</template>

<script setup lang="ts">
/**
 * ChannelTestForm — 上行通道测试表单（展示型组件）
 *
 * 由 ConnectionTest 容器传入 channels（含 _status/_latency/_error/_detail 测试状态字段），
 * 本组件只负责渲染表格与状态标签，测试逻辑通过 emit 上抛容器：
 *   - test-one: 单行"测试"按钮点击，payload = ChannelRow
 *   - test-all: "测试全部通道"按钮点击
 */
import { h } from 'vue'
import { ElButton, ElTag } from 'element-plus'
import { Promotion } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import type { ChannelRow } from './types'

defineProps<{
  channels: ChannelRow[]
  testing: boolean
}>()

const emit = defineEmits<{
  'test-one': [ch: ChannelRow]
  'test-all': []
}>()

/** 当前连接状态标签：与迁移前的 v-if 分支一一对应 */
function connectedTag(connected: ChannelRow['connected']) {
  if (connected === true) return h(ElTag, { type: 'success', size: 'small' }, () => '已连接')
  if (connected === false) return h(ElTag, { type: 'danger', size: 'small' }, () => '未连接')
  return h(ElTag, { type: 'info', size: 'small' }, () => '未知')
}

/** 测试状态标签：与迁移前的 v-if 分支一一对应 */
function statusTag(status: ChannelRow['_status']) {
  if (status === 'ok') return h(ElTag, { type: 'success', size: 'small' }, () => '通过')
  if (status === 'fail') return h(ElTag, { type: 'danger', size: 'small' }, () => '失败')
  if (status === 'testing') return h(ElTag, { type: 'warning', size: 'small' }, () => '测试中')
  return h(ElTag, { type: 'info', size: 'small' }, () => '未测试')
}

/** 详情列：错误信息 > 测试通过 > 占位符 */
function detailCell(row: ChannelRow) {
  if (row._error) return h('span', { class: 'ct-error' }, row._error)
  if (row._status === 'ok') return h('span', { class: 'ct-ok' }, row._detail || '发送成功')
  return h('span', { class: 'ct-muted' }, '-')
}

// 通道表统一走 DataTable；列顺序/宽度与迁移前保持一致
const columns: ColumnDef<ChannelRow>[] = [
  { key: 'name', title: '通道名称', minWidth: 140 },
  { key: 'type', title: '类型', width: 100 },
  { key: 'connected', title: '当前连接', width: 100, cell: (row) => connectedTag(row.connected) },
  { key: 'queue_length', title: '队列积压', width: 100, cell: (row) => row.queue_length ?? '-' },
  { key: 'status', title: '测试状态', width: 100, cell: (row) => statusTag(row._status) },
  {
    key: 'latency',
    title: '耗时',
    width: 100,
    align: 'right',
    cell: (row) => (row._latency != null ? `${row._latency}ms` : '-'),
  },
  { key: 'detail', title: '详情', minWidth: 200, cell: (row) => detailCell(row) },
  actionColumn<ChannelRow>({
    width: 100,
    fixed: 'right',
    cell: (row) =>
      h(
        ElButton,
        {
          size: 'small',
          type: 'primary',
          link: true,
          loading: row._status === 'testing',
          onClick: () => emit('test-one', row),
        },
        () => '测试',
      ),
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