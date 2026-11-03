<template>
  <div class="trash">
    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="已删除点位" icon="Delete">
          <template #extra>
            <el-tag type="info" effect="plain" round size="small" class="count-badge">
              {{ items.length }} 个
            </el-tag>
          </template>
          <el-tooltip content="刷新已删除点位" placement="bottom">
            <el-button size="small" aria-label="刷新" @click="load"><el-icon><Refresh /></el-icon></el-button>
          </el-tooltip>
          <!-- 与页签 / 区块标题统一叫「已删除点位」，不再混用「回收站」 -->
          <el-tooltip :disabled="items.length > 0" content="暂无已删除点位，删除点位后才会出现可清空项" placement="bottom">
            <span>
              <el-button
                type="danger"
                size="small"
                :disabled="items.length === 0 || clearing"
                :loading="clearing"
                @click="clearAll"
              >
                <el-icon><Delete /></el-icon> 清空已删除点位
              </el-button>
            </span>
          </el-tooltip>
        </SectionHeader>
      </template>

      <DataTable
        :data="items"
        :columns="columns"
        :row-key="rowKey"
        stripe
        :loading="loading"
        empty-text="暂无已删除点位，删除的点位会暂存 30 天后自动清理"
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h, onMounted, onUnmounted, ref } from 'vue'
import { ElButton, ElMessage } from 'element-plus'
import { Delete, Refresh } from '@element-plus/icons-vue'
import { confirmAction } from '@/utils/confirm'
import { listPointTrash, purgeAllTrash, purgeTrashPoint, restorePoint } from '@/api/points'
import type { PointTrashEntry } from '@/api/points'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import { formatDateTime } from '@/utils/datetime'

const items = ref<PointTrashEntry[]>([])
const loading = ref(false)
const clearing = ref(false)
let unmounted = false

/** 后端错误优先取响应体 message/error；包装层业务码错误已由拦截器转成 Error(message) */
function backendMessage(e: unknown, fallback: string): string {
  const data = (e as { response?: { data?: { error?: unknown; message?: unknown } } })?.response?.data
  const msg = data?.error ?? data?.message
  if (typeof msg === 'string' && msg.trim()) return msg
  if (e instanceof Error && e.message) return e.message
  return fallback
}

async function load() {
  loading.value = true
  try {
    const data = await listPointTrash()
    if (unmounted) return
    items.value = data
  } catch {
    /* 读取失败由全局拦截器留痕，这里不再重复弹窗 */
  } finally {
    if (!unmounted) loading.value = false
  }
}

async function restore(row: PointTrashEntry) {
  try {
    await restorePoint(row.point.id)
    if (unmounted) return
    ElMessage.success('已恢复')
    load()
  } catch (e) {
    // id 已被占用时后端会拒绝恢复，把后端 message 原样透出
    if (!unmounted) ElMessage.error(backendMessage(e, `恢复点位 ${row.point.id} 失败，请稍后重试`))
  }
}

async function purge(row: PointTrashEntry) {
  const ok = await confirmAction({
    message: `确定彻底删除点位「${row.point.id}」？此操作不可恢复`,
    title: '彻底删除',
    type: 'error',
    confirmText: '彻底删除',
  })
  if (!ok || unmounted) return
  try {
    await purgeTrashPoint(row.point.id)
    if (unmounted) return
    ElMessage.success('已彻底删除')
    load()
  } catch (e) {
    if (!unmounted) ElMessage.error(backendMessage(e, `彻底删除点位 ${row.point.id} 失败，请稍后重试`))
  }
}

async function clearAll() {
  if (items.value.length === 0) return
  const count = items.value.length
  const ok = await confirmAction({
    message: `确定清空已删除点位中的 ${count} 个点位？清空后不可恢复`,
    title: '清空已删除点位',
    type: 'error',
    confirmText: '清空',
  })
  if (!ok || unmounted) return
  clearing.value = true
  try {
    await purgeAllTrash()
    if (unmounted) return
    ElMessage.success('已删除点位已清空')
    await load()
  } catch (e) {
    if (!unmounted) ElMessage.error(backendMessage(e, '清空已删除点位失败，请稍后重试'))
  } finally {
    if (!unmounted) clearing.value = false
  }
}

const columns: ColumnDef<PointTrashEntry>[] = [
  {
    key: 'id',
    title: '点位 ID',
    minWidth: 180,
    ellipsis: true,
    cell: (row) => h('span', { class: 'cell-mono' }, row.point.id),
  },
  {
    key: 'name',
    title: '名称',
    width: 160,
    ellipsis: true,
    cell: (row) => row.point.name || '—',
  },
  {
    key: 'device_id',
    title: '所属设备',
    width: 160,
    ellipsis: true,
    cell: (row) => row.point.device_id || '—',
  },
  {
    key: 'protocol_address',
    title: '协议 / 地址',
    minWidth: 180,
    ellipsis: true,
    cell: (row) => `${row.point.protocol} / ${row.point.address}`,
  },
  {
    key: 'deleted_at_ms',
    title: '删除时间',
    width: 180,
    cell: (row) => formatDateTime(row.deleted_at_ms),
  },
  actionColumn<PointTrashEntry>({
    width: 170,
    fixed: 'right',
    cell: (row) =>
      h('div', null, [
        h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => restore(row) }, () => '恢复'),
        h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => purge(row) }, () => '彻底删除'),
      ]),
  }),
]

const rowKey = (row: PointTrashEntry) => row.point.id

onMounted(load)
onUnmounted(() => { unmounted = true })
</script>

<style scoped>
.trash {
  display: flex;
  flex-direction: column;
  gap: 20px;
}
.section-card {
  border-radius: var(--radius-card);
}
.count-badge {
  margin-left: 4px;
  font-weight: 400;
}
</style>
