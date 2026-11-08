<template>
  <el-card shadow="never" class="section-card">
    <template #header>
      <SectionHeader title="通知重试队列（本机浏览器）" icon="Bell">
        <template #extra>
          <LocalOnlyBadge label="仅本机" tip="浏览器本地通知重试，非设备数据" />
          <el-tag :type="items.length ? 'warning' : 'info'" effect="dark" round size="small">
            {{ items.length }} 条
          </el-tag>
        </template>
        <el-button size="small" type="primary" :disabled="!items.length" @click="$emit('retry-all')">
          <el-icon><Promotion /></el-icon> 全部重发
        </el-button>
        <el-button size="small" type="danger" :disabled="!items.length" @click="$emit('clear')">
          <el-icon><Delete /></el-icon> 清空
        </el-button>
      </SectionHeader>
    </template>

    <p class="notify-desc">
      仅记录本机浏览器发出的 Webhook 通知发送失败后的重试项。它不是设备端数据死信，不随设备迁移，换一台电脑查看即为空。
    </p>

    <DataTable
      :data="items"
      :columns="columns"
      row-key="id"
      stripe
      empty-text="本机暂无通知重试项"
    />
  </el-card>
</template>

<script setup lang="ts">
/**
 * 通知重试队列面板（仅本机浏览器）
 *
 * 数据来自 localStorage（key: gw_webhook_notify_queue），只代表「这台电脑」
 * 发通知失败的重试项；设备端未送达数据请看 DeviceDeadLetterPanel。
 * 纯展示 + 事件上抛，不自行读写队列，避免与 useWebhook 的单例状态分叉。
 */
import { h } from 'vue'
import { ElButton } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'

interface NotifyQueueRow {
  id: string
  payload: { event: string; message: string }
  retryCount: number
  lastError?: string
}

defineProps<{ items: NotifyQueueRow[] }>()

const emit = defineEmits<{
  /** 重发单条通知 */
  (e: 'retry', id: string): void
  /** 重发全部通知 */
  (e: 'retry-all'): void
  /** 清空本机通知重试队列 */
  (e: 'clear'): void
}>()

const columns: ColumnDef<NotifyQueueRow>[] = [
  { key: 'payload.event', title: '事件', width: 140, cell: (row) => row.payload.event },
  { key: 'payload.message', title: '消息', minWidth: 200, ellipsis: true, cell: (row) => row.payload.message },
  { key: 'retryCount', title: '重试次数', width: 90 },
  { key: 'lastError', title: '错误', minWidth: 160, ellipsis: true },
  actionColumn<NotifyQueueRow>({
    width: 90,
    cell: (row) => h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => emit('retry', row.id) }, () => '重发'),
  }),
]
</script>

<style scoped>
.notify-desc {
  margin: 0 0 12px;
  font-size: 0.8125rem;
  color: var(--text-secondary);
  line-height: 1.6;
}
</style>
