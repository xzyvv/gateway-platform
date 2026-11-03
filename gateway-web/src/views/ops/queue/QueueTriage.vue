<template>
  <div class="queue-triage">
    <SectionHeader title="队列排障" icon="Cpu">
      <el-button size="small" @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
    </SectionHeader>

    <!-- R5：模拟消费者不再是系统域的独立入口，收进运维监控的「队列排障」，
         并把「先看队列、再决定是否启动兜底消费者」的排查顺序写在页面上 -->
    <el-alert
      class="triage-hint"
      type="info"
      show-icon
      :closable="false"
      title="用于区分「没有消费者」与「上行链路故障」"
      description="先在「运行指标」看队列长度是否持续增长；没有可用 MQTT 通道时，可启动模拟消费者把队列数据出队、处理并 ack（不外发任何数据），以确认积压是否由消费者缺失导致。MQTT 已启用时与模拟消费者互斥。"
    />

    <!-- 状态区：运行态（running）与配置意图（enabled）分开呈现，
         两者含义不同：enabled 只说明「配置开了」，不代表「真的在跑」。 -->
    <el-card shadow="never" class="section-card">
      <template #header>
        <strong>模拟消费者运行状态</strong>
      </template>

      <el-alert
        v-if="blockedByMqtt"
        title="MQTT 已启用，模拟消费者不可用（两者会分流同一批数据）"
        type="warning"
        show-icon
        :closable="false"
        class="mqtt-alert"
      />

      <el-alert
        v-if="!canWrite"
        title="当前角色无配置写权限（config:write），仅可查看运行状态"
        type="info"
        show-icon
        :closable="false"
        class="readonly-alert"
      />

      <el-descriptions :column="2" border size="small">
        <el-descriptions-item label="运行状态">
          <el-tag :type="running ? 'success' : 'info'" effect="dark" size="small">
            {{ running ? '运行中' : '已停用' }}
          </el-tag>
        </el-descriptions-item>
        <el-descriptions-item label="配置意图">
          <el-tooltip content="[sim_consumer].enabled：网关启动时是否自动拉起模拟消费者" placement="top">
            <el-tag :type="enabled ? 'success' : 'info'" effect="plain" size="small">
              {{ enabled ? '已启用' : '未启用' }}
            </el-tag>
          </el-tooltip>
        </el-descriptions-item>
        <el-descriptions-item label="本次启动时刻">{{ startedAtText }}</el-descriptions-item>
        <el-descriptions-item label="阻塞原因">{{ blockedReasonText }}</el-descriptions-item>
      </el-descriptions>
    </el-card>

    <!-- 指标区 -->
    <el-card shadow="never" class="section-card">
      <template #header><strong>消费指标</strong></template>
      <el-row :gutter="16">
        <el-col :span="6"><StatCard label="已消费条数" :value="consumedText" /></el-col>
        <el-col :span="6"><StatCard label="消费批次数" :value="batchesText" /></el-col>
        <el-col :span="6"><StatCard label="当前队列长度" :value="queueLenText" /></el-col>
        <el-col :span="6"><StatCard label="运行时长" :value="uptimeText" /></el-col>
      </el-row>
      <div class="last-consumed">最近消费时刻：{{ lastConsumedText }}</div>
    </el-card>

    <!-- 参数表单：只在「启动」时随请求体提交；运行中不允许改参数 -->
    <el-card shadow="never" class="section-card">
      <template #header><strong>运行参数</strong></template>
      <el-form label-width="170px" size="small">
        <el-form-item label="单批出队条数">
          <el-input-number
            v-model="form.batchSize"
            :min="1"
            :max="100000"
            controls-position="right"
            :disabled="running"
            @change="formDirty = true"
          />
        </el-form-item>
        <el-form-item label="消费间隔(毫秒)">
          <el-input-number
            v-model="form.intervalMs"
            :min="10"
            :max="60000"
            controls-position="right"
            :disabled="running"
            @change="formDirty = true"
          />
        </el-form-item>
        <el-form-item label="日志打印间隔(条)">
          <el-input-number
            v-model="form.logEvery"
            :min="0"
            :max="100000"
            controls-position="right"
            :disabled="running"
            @change="formDirty = true"
          />
        </el-form-item>
        <el-form-item>
          <el-button
            type="primary"
            :loading="starting"
            :disabled="running || blockedByMqtt || !canWrite"
            @click="onStart"
          >
            启动
          </el-button>
          <el-button
            type="danger"
            :loading="stopping"
            :disabled="!running || !canWrite"
            @click="onStop"
          >
            停止
          </el-button>
          <span class="hint">运行参数只在启动时生效，运行中需先停止再修改</span>
        </el-form-item>
      </el-form>
    </el-card>
  </div>
</template>

<script setup lang="ts">
/**
 * 队列排障（运维监控 › 队列排障）
 *
 * 上行通道的兜底消费者：没有可用 MQTT 通道时，仍把队列数据走完
 * 「出队 → 处理 → ack」，消除「队列有积压却没有消费者」的告警歧义，
 * 但不外发任何数据。
 *
 * 与 MQTT 互斥：两者会分流同一批队列数据，被模拟消费者分走的那部分被消费掉
 * 却不会上报，现场表现为静默丢数。因此 MQTT 已启用时后端拒绝启动（HTTP 409），
 * 页面需把这一约束显式告知用户。
 */
import { computed, reactive, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { Refresh } from '@element-plus/icons-vue'
import StatCard from '@/components/StatCard.vue'
import { getSimConsumerStatus, startSimConsumer, stopSimConsumer } from '@/api/simConsumer'
import type { SimConsumerStatus } from '@/api/types'
import { usePolling } from '@/composables/core/usePolling'
import { usePermission } from '@/composables/auth/usePermission'
import { extractErrorMessage, failureMessage } from '@/utils/errorHandler'
import { formatDateTime } from '@/utils/datetime'

const status = ref<SimConsumerStatus | null>(null)
const starting = ref(false)
const stopping = ref(false)

/** 表单值（camelCase，与后端启动请求体一致） */
const form = reactive({ batchSize: 100, intervalMs: 1000, logEvery: 100 })
/** 用户是否改过表单：改过之后不再被轮询回来的后端默认值覆盖 */
const formDirty = ref(false)

/** 实际运行态 */
const running = computed(() => status.value?.running === true)
/** 配置意图（[sim_consumer].enabled） */
const enabled = computed(() => status.value?.enabled === true)
/** MQTT 互斥导致的阻塞 */
const blockedByMqtt = computed(() => status.value?.blocked_reason === 'mqtt_enabled')

// 启停属 config:write（后端 `/api/sim_consumer` 按该 scope 鉴权），
// 前端同口径隐藏动作，避免只读角色点了才被 403 拦下。
const { can } = usePermission()
const canWrite = computed(() => can('config.write'))

// ts-rs 把 u64 标成 bigint，运行时 axios 解析出来是 number，统一用 Number() 兜底
const consumedText = computed(() =>
  status.value ? String(Number(status.value.consumed)) : '-',
)
const batchesText = computed(() =>
  status.value ? String(Number(status.value.batches)) : '-',
)
const queueLenText = computed(() =>
  status.value ? `${Number(status.value.queue_len)} 条` : '-',
)
const uptimeText = computed(() => formatDuration(toNumber(status.value?.uptime_ms)))
const lastConsumedText = computed(() => formatTimestamp(toNumber(status.value?.last_consumed_at_ms)))
const startedAtText = computed(() => formatTimestamp(toNumber(status.value?.started_at_ms)))
const blockedReasonText = computed(() => {
  const reason = status.value?.blocked_reason
  if (!reason) return '-'
  return reason === 'mqtt_enabled' ? 'MQTT 已启用' : reason
})

/** bigint | number | null → number（ts-rs 把 u64 标成 bigint，运行时实际是 number） */
function toNumber(v: bigint | number | null | undefined): number | null {
  if (v == null) return null
  const n = Number(v)
  return Number.isFinite(n) ? n : null
}

/** 毫秒 → 可读时长（如「1 分 5 秒」） */
function formatDuration(ms: number | null): string {
  if (ms == null || !Number.isFinite(ms)) return '-'
  const total = Math.floor(ms / 1000)
  const d = Math.floor(total / 86400)
  const h = Math.floor((total % 86400) / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  if (d > 0) return `${d} 天 ${h} 小时 ${m} 分`
  if (h > 0) return `${h} 小时 ${m} 分 ${s} 秒`
  return `${m} 分 ${s} 秒`
}

/** epoch 毫秒 → 本地时间文本 */
function formatTimestamp(ms: number | null): string {
  if (ms == null || !Number.isFinite(ms)) return '-'
  return formatDateTime(ms) || '-'
}

/** 应用一次状态快照；force 为真时强制用后端值覆盖表单 */
function applyStatus(next: SimConsumerStatus, force = false) {
  status.value = next
  if (force) formDirty.value = false
  if (formDirty.value) return
  form.batchSize = Number(next.batch_size)
  form.intervalMs = Number(next.interval_ms)
  form.logEvery = Number(next.log_every)
}

/** 轮询加载：失败时静默（GET 失败由全局拦截器统一提示），避免每 2 秒刷屏 */
async function load() {
  try {
    applyStatus(await getSimConsumerStatus())
  } catch {
    // 忽略：保留上一次快照
  }
}

/**
 * 把后端错误翻译成可读提示：
 * 409 冲突（已在运行 / MQTT 已启用）给出直接可执行的结论，其余走通用兜底。
 */
function describeError(e: unknown, action: string): string {
  const detail = extractErrorMessage(e, '')
  if (detail.includes('MQTT')) return 'MQTT 已启用，无法启动模拟消费者'
  if (detail.includes('不可修改')) return '运行参数不可修改，请先停止模拟消费者'
  if (detail.includes('已在运行')) return '模拟消费者已在运行'
  if (detail.includes('未在运行')) return '模拟消费者未在运行'
  return failureMessage(action, e)
}

async function onStart() {
  starting.value = true
  try {
    applyStatus(
      await startSimConsumer({
        batchSize: form.batchSize,
        intervalMs: form.intervalMs,
        logEvery: form.logEvery,
      }),
      true,
    )
    ElMessage.success('模拟消费者已启动')
  } catch (e) {
    ElMessage.error(describeError(e, '启动模拟消费者'))
  } finally {
    starting.value = false
  }
}

async function onStop() {
  stopping.value = true
  try {
    applyStatus(await stopSimConsumer(), true)
    ElMessage.success('模拟消费者已停止')
  } catch (e) {
    ElMessage.error(describeError(e, '停止模拟消费者'))
  } finally {
    stopping.value = false
  }
}

// 每 2 秒轮询一次状态；usePolling 负责隐藏暂停与卸载清理（onUnmounted → stop）
usePolling(load, 2000)
</script>

<style scoped>
.queue-triage { padding: 0; }
.triage-hint { margin-bottom: 16px; }
.section-card { margin-bottom: 16px; }
.mqtt-alert { margin-bottom: 12px; }
.readonly-alert { margin-bottom: 12px; }
.last-consumed { margin-top: 12px; font-size: 0.8125rem; color: var(--el-text-color-secondary); }
.hint { margin-left: 8px; color: var(--el-text-color-secondary); font-size: 0.75rem; }
</style>
