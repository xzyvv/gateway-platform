<template>
  <el-card shadow="never" class="section-card device-dlq-panel">
    <template #header>
      <SectionHeader title="设备端死信（未送达数据）" icon="WarningFilled">
        <template #extra>
          <!-- 关键语义：接口失败时状态「未知」，绝不能显示成 0 条 -->
          <el-tag v-if="error" type="warning" effect="dark" round size="small">状态未知</el-tag>
          <el-tag v-else-if="loading && total === null" type="info" effect="plain" round size="small">读取中…</el-tag>
          <el-tag v-else-if="total === 0" type="success" effect="dark" round size="small">无未送达数据</el-tag>
          <el-tag v-else type="danger" effect="dark" round size="small">{{ total }} 条</el-tag>
        </template>
        <el-button size="small" :loading="loading" @click="load">
          <el-icon><Refresh /></el-icon> 刷新
        </el-button>
        <el-button size="small" type="danger" :disabled="!canPurge" @click="purge">
          <el-icon><Delete /></el-icon> 清理
        </el-button>
      </SectionHeader>
    </template>

    <p class="dlq-desc">
      Webhook 上报被对端永久拒绝（4xx）或重试用尽的数据批次，保存在网关本地，可修复对端后重投。
    </p>

    <el-alert v-if="error" type="error" :closable="false" show-icon :title="error" />

    <!-- 自动裁剪过的数据已经**永久删除**：不提示的话，界面只是一张"正常的表"，
         运维会以为历史上从未丢过数据 -->
    <el-alert
      v-else-if="trimmedTotal > 0"
      type="warning"
      :closable="false"
      show-icon
      class="dlq-trim"
      :title="`死信表超过行数上限，已自动永久删除 ${trimmedTotal} 条历史记录`"
      :description="trimDesc"
    />

    <DataTable
      v-if="!error"
      :data="items"
      :columns="columns"
      row-key="id"
      stripe
      :loading="loading"
      :page="page"
      :page-size="pageSize"
      :total="total ?? undefined"
      empty-text="无未送达数据"
      @update:page="onPageChange"
    />
  </el-card>
</template>

<script setup lang="ts">
/**
 * 设备端死信面板（网关本地持久化的未送达数据）
 *
 * 与 `通知重试队列（本机浏览器）` 严格区分：这里的数据在网关设备上，
 * 换台电脑查看仍然存在；接口 503 时表示「查不到」，必须显示错误，
 * 不能退化成「0 条」（否则运维会误判为从未丢过数据）。
 */
import { computed, h, onMounted, ref } from 'vue'
import { ElButton, ElMessage } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import { formatDateTime } from '@/utils/datetime'
import { actionColumn, timeColumn } from '@/utils/commonColumns'
import { confirmAction } from '@/utils/confirm'
import { extractErrorMessage, failureMessage } from '@/utils/errorHandler'
import {
  listDeadLetters,
  purgeDeadLetters,
  replayDeadLetter,
  type DeadLetterItem,
} from '@/api/deadLetter'

const PAGE_SIZE = 50

const items = ref<DeadLetterItem[]>([])
/** null = 条数未知（加载中或接口失败）；不能当作 0 */
const total = ref<number | null>(null)
/** 因超上限被自动裁剪的累计条数（0 = 从未裁剪） */
const trimmedTotal = ref(0)
/** 最近一次自动裁剪的时间（epoch 毫秒，0 = 从未裁剪） */
const lastTrimMs = ref(0)
const lastTrimRows = ref(0)
const loading = ref(false)
const error = ref('')
const page = ref(1)
const pageSize = ref(PAGE_SIZE)

const canPurge = computed(() => !error.value && (total.value ?? 0) > 0)

/** 自动裁剪的补充说明：什么时候、删了多少 */
const trimDesc = computed(() => {
  if (!lastTrimMs.value) return '更早的死信已无法恢复，如需留存请下调上报量或及时重投/清理。'
  return `最近一次自动裁剪：${formatDateTime(lastTrimMs.value)}，删除 ${lastTrimRows.value} 条。这些数据已不可恢复。`
})

function httpStatus(e: unknown): number | undefined {
  return (e as { response?: { status?: number } } | null | undefined)?.response?.status
}

async function load() {
  loading.value = true
  error.value = ''
  try {
    const res = await listDeadLetters({
      limit: pageSize.value,
      offset: (page.value - 1) * pageSize.value,
    })
    items.value = res.items ?? []
    total.value = res.total ?? 0
    trimmedTotal.value = res.trimmed_total ?? 0
    lastTrimMs.value = res.last_trim_ms ?? 0
    lastTrimRows.value = res.last_trim_rows ?? 0
  } catch (e) {
    // 失败时清空行并置 total=null：保留旧数字会让「未知」被读成「已知」
    items.value = []
    total.value = null
    trimmedTotal.value = 0
    const detail = extractErrorMessage(e).trim()
    const suffix = detail ? `：${detail}` : ''
    error.value = httpStatus(e) === 503
      ? `死信库不可用，无法确认是否存在未送达数据${suffix}`
      : `加载设备端死信失败，无法确认是否存在未送达数据${suffix}`
  } finally {
    loading.value = false
  }
}

function onPageChange(p: number) {
  page.value = p
  void load()
}

async function replay(row: DeadLetterItem) {
  try {
    const res = await replayDeadLetter(row.id)
    if (res.requeued) ElMessage.success(`已重新入队（outbox #${res.outbox_id}）`)
    else ElMessage.warning('重投未生效：该批次已有在途记录')
    await load()
  } catch (e) {
    // 409（code=4009）等业务错误：把后端 message 原样展示，便于现场判断
    ElMessage.error(failureMessage('重投', e))
  }
}

async function purge() {
  const ok = await confirmAction({
    message: '清理后这些未送达数据将从网关本地永久删除且不可恢复，确认清理全部设备端死信？',
    title: '清理设备端死信',
    type: 'warning',
    confirmText: '清理',
  })
  if (!ok) return
  try {
    const res = await purgeDeadLetters()
    ElMessage.success(`已清理 ${res.deleted} 条死信`)
    page.value = 1
    await load()
  } catch (e) {
    ElMessage.error(failureMessage('清理', e))
  }
}

const columns: ColumnDef<DeadLetterItem>[] = [
  { key: 'data_id', title: 'data_id', width: 110 },
  { key: 'reason', title: '失败原因', width: 150, ellipsis: true },
  { key: 'attempt_count', title: '尝试次数', width: 100 },
  timeColumn<DeadLetterItem>('failed_at', { title: '失败时间', width: 170 }),
  { key: 'payload_preview', title: '数据预览', minWidth: 240, ellipsis: true },
  actionColumn<DeadLetterItem>({
    width: 90,
    cell: (row) => h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => replay(row) }, () => '重投'),
  }),
]

onMounted(load)
</script>

<style scoped>
.dlq-desc {
  margin: 0 0 12px;
  font-size: 0.8125rem;
  color: var(--text-secondary);
  line-height: 1.6;
}

.dlq-trim {
  margin-bottom: 12px;
}
</style>
