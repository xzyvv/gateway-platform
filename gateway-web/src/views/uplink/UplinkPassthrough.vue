<template>
  <div class="pt-sessions">
    <SectionHeader title="透传会话" icon="Share">
<template #extra>
<el-tag v-if="data" size="small" :type="data.enabled ? 'success' : 'info'" effect="plain" round>
          {{ data.enabled ? '透传模式已启用' : '透传模式未启用' }}
          </el-tag>
        </template>
      </SectionHeader>

    <div class="top-bar">
      <el-button type="primary" @click="load" :loading="loading">
        <el-icon><Refresh /></el-icon> 刷新
      </el-button>
      <el-tooltip content="每 5 秒自动刷新" placement="top">
        <el-switch
          v-model="autoRefresh"
          active-text="自动刷新"
          @change="toggleAuto"
        />
      </el-tooltip>
    </div>

    <!-- 骨架分支：DataTable 同构占位，避免 v-loading 蒙层 -->
    <template v-if="loading">
      <TableSkeleton :rows="6" :columns="5" label="正在加载透传会话" />
    </template>

    <template v-else>
      <empty-state
        v-if="(!data || !data.enabled)"
        description="透传模式未启用"
        action-text="去运行参数配置"
        @action="goRuntimeConfig"
      >
        <p class="pt-hint">在「系统 › 运行参数 › 原始 JSON」中配置 modbus_passthrough 段并将 enabled 设为 true，保存应用后活跃会话会出现在这里。</p>
      </empty-state>

      <template v-else>
        <DataTable
          :data="data?.sessions ?? []"
          :columns="sessionColumns"
          row-key="id"
          stripe
          empty-text="暂无透传数据"
        />

        <div class="session-summary" v-if="data && data.sessions.length > 0">
          <el-tag size="small" type="info" effect="plain" round>
            共 {{ data.sessions.length }} 个活跃会话
          </el-tag>
        </div>
      </template>
    </template>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, h } from 'vue'
import { useRouter } from 'vue-router'
import { usePolling } from '@/composables/core/usePolling'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import {Refresh} from '@element-plus/icons-vue'
import { getPassthroughSessions, type PassthroughSession } from '@/api/uplink'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import { logError } from '@/utils/logger'
import TableSkeleton from '@/components/TableSkeleton.vue'
import { formatDateTime } from '@/utils/datetime'

interface PtData {
  enabled: boolean
  sessions: PassthroughSession[]
}

// DataTable 列定义：宽度/对齐与原列保持一致
const sessionColumns: ColumnDef<PassthroughSession>[] = [
  { key: 'id', title: '会话 ID', minWidth: 180, ellipsis: true },
  { key: 'bus', title: '总线', minWidth: 120 },
  { key: 'started_at', title: '开始时间', minWidth: 180, cell: (row) => h('span', formatTime(row.started_at)) },
  { key: 'timeout_ms', title: '超时 (ms)', minWidth: 120, align: 'right' },
  actionColumn<PassthroughSession>({ key: 'op', width: 80, align: 'center', cell: () => h('span', { class: 'no-op' }, '-') }),
]

const data = ref<PtData | null>(null)
const loading = ref(false)
const autoRefresh = ref(false)
const router = useRouter()

/** 空态引导：跳到运行参数页（透传开关在「原始 JSON」的 modbus_passthrough 段） */
function goRuntimeConfig() {
  void router.push({ path: '/system/config', query: { tab: 'runtime-config' } })
}

function formatTime(t: number) {
  if (!t) return '-'
  const ms = t > 1e12 ? t : t * 1000
  return formatDateTime(ms)
}

async function load() {
  loading.value = true
  try {
    data.value = await getPassthroughSessions()
  } catch (e) {
    showToast(e, '加载透传会话失败')
    logError('uplink-pass', e)
  } finally {
    loading.value = false
  }
}

// 自动刷新轮询：usePolling 统一提供隐藏暂停 / 失败退避 / 卸载清理
const { start: startPoll, stop: stopPoll } = usePolling(load, 5000, { autoStart: false })

function toggleAuto(val: boolean) {
  if (val) {
    startPoll()
  } else {
    stopPoll()
  }
}

onMounted(load)
</script>

<style scoped>
.pt-sessions { padding: 0px; }
.top-bar { display: flex; gap: 16px; align-items: center; margin-bottom: 16px; }
.no-op { color: var(--text-secondary); }
.session-summary { margin-top: 12px; }
.pt-hint { margin: 8px 0 0; font-size: 0.8125rem; color: var(--text-secondary); line-height: 1.6; max-width: 420px; }
</style>
