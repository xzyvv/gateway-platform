<template>
  <div class="north-status">
    <!-- W80: 与 VISUAL_SPEC §8.2 一致——使用 card-header + section-header::before 渐变装饰条（保持原 div 结构避免 v-loading 嵌套问题） -->
    <SectionHeader title="上行服务状态" icon="Share">
      <template #extra>
        <el-tag type="info" effect="plain" round size="small" class="count-badge">
          共 {{ status.channels.length }} 个上行通道
        </el-tag>
      </template>
      <el-button type="primary" size="small" @click="load" :loading="loading">
      <el-icon><Refresh /></el-icon> 刷新
      </el-button>
      <el-tooltip content="每 10 秒自动刷新" placement="top">
      <el-switch v-model="autoRefresh" active-text="自动刷新" @change="toggleAuto" />
      </el-tooltip>
    </SectionHeader>

    <!-- 骨架分支：与真实卡片区等宽等高占位，数据到达时布局不跳 -->
    <template v-if="loading">
      <el-skeleton animated class="sk-health">
        <template #template>
          <el-skeleton-item variant="rect" class="sk-health-box" />
        </template>
      </el-skeleton>
      <el-row :gutter="16" v-if="status.aggregated" class="sk-agg">
        <el-col :span="4" v-for="i in 4" :key="`agg-${i}`">
          <el-skeleton animated class="sk-stat-card">
            <template #template>
              <el-skeleton-item variant="rect" class="sk-stat-box" />
            </template>
          </el-skeleton>
        </el-col>
      </el-row>
      <el-row :gutter="16">
        <el-col :xs="24" :sm="12" :md="8" v-for="i in 6" :key="`ch-${i}`">
          <el-card shadow="never" class="channel-card">
            <el-skeleton animated>
              <template #template>
                <el-skeleton-item variant="text" class="sk-head-line" />
                <el-skeleton-item variant="text" class="sk-body-line" />
                <el-skeleton-item variant="text" class="sk-body-line" />
                <el-skeleton-item variant="text" class="sk-body-line short" />
              </template>
            </el-skeleton>
          </el-card>
        </el-col>
      </el-row>
    </template>

    <template v-else>
      <!-- C2: 统一结论。现场第一个问题是「上行到底通没通」，这里把通道 / Server /
           透传三方状态折算成一句结论 + 问题清单，原始明细在下方卡片中。 -->
      <el-alert
        :type="verdictAlertType(health.verdict)"
        :closable="false"
        show-icon
        class="mb-16 health-alert"
        :title="`${health.title}｜${health.summary}`"
      >
        <ul v-if="health.issues.length > 0" class="health-issues">
          <li v-for="(it, i) in health.issues" :key="i" :class="`issue-${it.level}`">
            {{ it.text }}
          </li>
        </ul>
      </el-alert>

      <el-row :gutter="16" v-if="status.aggregated">
        <el-col :span="4" v-for="d in aggStats" :key="d.label">
          <StatCard :label="d.label" :value="d.value" :color="d.color" class="mb-16" />
        </el-col>
      </el-row>

      <!-- 扇出订阅 Lagged 丢帧：通道消费不及导致批次被丢弃，属真实数据缺口，必须显式提示 -->
      <el-alert
        v-if="laggedDropped > 0"
        type="warning"
        :closable="false"
        show-icon
        class="mb-16"
        :title="`上行扇出累计丢帧 ${laggedDropped} 批次（通道处理不及，请检查北向连通性与处理能力）`"
      />

      <el-row :gutter="16">
        <el-col :xs="24" :sm="12" :md="8" v-for="ch in status.channels" :key="ch.name">
          <el-card shadow="never" class="channel-card">
            <div class="channel-head">
              <div class="head-left">
                <el-icon :style="{ color: typeColor(ch.type), fontSize: '1.125rem' }">
                  <component :is="typeIcon(ch.type)" />
                </el-icon>
                <span class="channel-name">{{ ch.name }}</span>
              </div>
              <!-- 配置态（是否启用）与运行态（是否连上）分开表达，避免「已启用」被读成「数据在发」 -->
              <el-tag v-if="!ch.enabled" size="small" type="info" effect="dark" round>已停用</el-tag>
              <el-tag v-else-if="ch.connected === true" size="small" type="success" effect="dark" round>已启用·已连接</el-tag>
              <el-tag v-else-if="ch.connected === false" size="small" type="danger" effect="dark" round>已启用·未连接</el-tag>
              <el-tag v-else size="small" type="warning" effect="dark" round>已启用·无连接态</el-tag>
            </div>

            <div class="channel-body">
              <div class="channel-meta">
                <span class="meta-label">类型</span>
                <span class="meta-value">{{ ch.type }}</span>
              </div>
              <div class="channel-meta">
                <span class="meta-label">队列长度</span>
                <span class="meta-value" :class="{ 'queue-warn': ch.queue_length! > 100 }">
                  {{ ch.queue_length! }}
                </span>
              </div>

              <!-- 最近错误：连接器真实记录（成功后自动清空），排障时比「未连接」更有信息量 -->
              <div v-if="ch.extra.last_error" class="channel-meta">
                <span class="meta-label">最近错误</span>
                <span class="meta-value last-error" :title="ch.extra.last_error">{{ ch.extra.last_error }}</span>
              </div>

              <!-- MQTT 专属 -->
              <template v-if="ch.type === 'mqtt'">
                <div class="channel-meta">
                  <span class="meta-label">Broker</span>
                  <span class="meta-value broker">{{ ch.extra.broker || '-' }}</span>
                </div>
                <div class="channel-meta">
                  <span class="meta-label">断路器</span>
                  <el-tag :type="ch.extra.circuit_open ? 'danger' : 'success'" size="small" effect="plain">
                    {{ ch.extra.circuit_open ? '已熔断' : '正常' }}
                  </el-tag>
                </div>
              </template>

              <!-- Webhook 专属 -->
              <template v-else-if="ch.type === 'http'">
                <div class="channel-meta">
                  <span class="meta-label">成功/失败</span>
                  <span class="meta-value">
                    <span class="ok">{{ ch.extra.success_count || 0 }}</span>
                    /
                    <span class="fail">{{ ch.extra.failed_count || 0 }}</span>
                  </span>
                </div>
                <div class="channel-meta">
                  <span class="meta-label">平均延迟</span>
                  <span class="meta-value">{{ ch.extra.avg_latency_ms != null ? ch.extra.avg_latency_ms + ' ms' : '-' }}</span>
                </div>
              </template>
            </div>
          </el-card>
        </el-col>
      </el-row>

      <!-- 对外服务 Server 存活状态（OPC UA / Modbus / 透传），与上行通道不是同一概念 -->
      <template v-if="servers.length > 0">
        <SectionHeader title="对外服务 Server" icon="Cpu" class="server-section-header">
          <template #extra>
            <el-tag size="small" :type="servers.every(s => s.running) ? 'success' : 'warning'" effect="plain" round>
              {{ servers.filter(s => s.running).length }}/{{ servers.length }} 运行中
            </el-tag>
          </template>
        </SectionHeader>
        <el-row :gutter="16">
          <el-col :xs="24" :sm="12" :md="8" v-for="srv in servers" :key="srv.name">
            <el-card shadow="never" class="channel-card">
              <div class="channel-head">
                <div class="head-left">
                  <el-icon :style="{ color: srv.running ? 'var(--text-success)' : 'var(--text-danger)', fontSize: '1.125rem' }">
                    <Cpu />
                  </el-icon>
                  <span class="channel-name">{{ srv.name }}</span>
                </div>
                <el-tag v-if="srv.running" size="small" type="success" effect="dark" round>运行中</el-tag>
                <el-tag v-else size="small" type="danger" effect="dark" round>已停止</el-tag>
              </div>
            </el-card>
          </el-col>
        </el-row>
      </template>

      <!-- C2: 透传会话也属于北向健康口径（上位机可通过北向报文直通南向总线），
           这里给出启用状态与活动会话概况，完整会话表在「透传会话」页签。 -->
      <SectionHeader title="透传会话" icon="Connection" class="server-section-header">
        <template #extra>
          <el-tag size="small" :type="passthrough.enabled ? 'success' : 'info'" effect="plain" round>
            {{ passthrough.enabled ? '已启用' : '未启用' }} · {{ passthrough.sessions.length }} 个活动会话
          </el-tag>
        </template>
      </SectionHeader>
      <el-alert
        v-if="!passthrough.enabled"
        type="info"
        :closable="false"
        show-icon
        class="mb-16"
        title="透传模式未启用：上位机无法通过北向报文直通南向总线。如需启用请到「系统 › 运行参数」配置 modbus_passthrough。"
      />
      <el-alert
        v-else-if="passthrough.sessions.length === 0"
        type="info"
        :closable="false"
        show-icon
        class="mb-16"
        title="透传模式已启用，当前无活动会话。"
      />
      <el-row v-else :gutter="16">
        <el-col :xs="24" :sm="12" :md="8" v-for="s in passthrough.sessions" :key="s.id">
          <el-card shadow="never" class="channel-card">
            <div class="channel-head">
              <div class="head-left">
                <el-icon :style="{ color: 'var(--color-primary)', fontSize: '1.125rem' }">
                  <Connection />
                </el-icon>
                <span class="channel-name">{{ s.id }}</span>
              </div>
              <el-tag size="small" type="primary" effect="dark" round>会话中</el-tag>
            </div>
            <div class="channel-body">
              <div class="channel-meta">
                <span class="meta-label">总线</span>
                <span class="meta-value">{{ s.bus }}</span>
              </div>
              <div class="channel-meta">
                <span class="meta-label">开始时间</span>
                <span class="meta-value">{{ formatSessionTime(s.started_at) }}</span>
              </div>
              <div class="channel-meta">
                <span class="meta-label">超时</span>
                <span class="meta-value">{{ s.timeout_ms }} ms</span>
              </div>
            </div>
          </el-card>
        </el-col>
      </el-row>
    </template>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, computed, onMounted, onUnmounted } from 'vue'
import { usePolling } from '@/composables/core/usePolling'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { Share, Refresh, Connection, Files } from '@element-plus/icons-vue'
import {
  getNorthStatus,
  getPassthroughSessions,
  type NorthChannel,
  type NorthAggregated,
  type NorthServer,
  type PassthroughSession,
} from '@/api/uplink'
import { computeNorthHealth, verdictAlertType } from '@/utils/northHealth'
import { logError } from '@/utils/logger'
import { formatClock, formatDateTime } from '@/utils/datetime'

const status = reactive<{ channels: NorthChannel[]; aggregated?: NorthAggregated }>({
  channels: [],
})
const servers = ref<NorthServer[]>([])
const passthrough = reactive<{ enabled: boolean; sessions: PassthroughSession[] }>({
  enabled: false,
  sessions: [],
})
const loading = ref(false)
const autoRefresh = ref(false)
const lastUpdate = ref('')

const laggedDropped = computed(() => status.aggregated?.lagged_dropped_batches || 0)

/** C2: 通道 / 对外服务 / 透传会话统一折算成一个结论 */
const health = computed(() =>
  computeNorthHealth({
    channels: status.channels,
    servers: servers.value,
    passthrough: { enabled: passthrough.enabled, sessions: passthrough.sessions },
    laggedDropped: laggedDropped.value,
    pendingMessages: status.aggregated?.pending_messages ?? 0,
  }),
)

/** 会话开始时间：后端可能给秒或毫秒，统一按毫秒展示 */
function formatSessionTime(t: number) {
  if (!t) return '-'
  return formatDateTime(t > 1e12 ? t : t * 1000)
}

const aggStats = computed(() => {
  if (!status.aggregated) return []
  const a = status.aggregated
  const all = a.mqtt + a.http
  const connected = a.mqtt_connected + a.http_connected
  return [
    { label: '上行通道总数', value: all, color: 'var(--text-primary)' },
    { label: '已连接', value: connected, color: 'var(--text-success)' },
    { label: '消息队列积压', value: a.pending_messages, color: 'var(--text-warning)' },
    { label: '上行消息总数', value: a.total_messages_sent, color: 'var(--color-primary)' },
  ]
})

function typeColor(t?: string) {
  switch (t) {
    case 'mqtt': return 'var(--color-primary)'
    case 'http': return 'var(--color-success)'
    default: return 'var(--text-secondary)'
  }
}

function typeIcon(t?: string) {
  switch (t) {
    case 'mqtt': return Connection
    case 'http': return Files
    default: return Share
  }
}

async function load() {
  loading.value = true
  try {
    // 两个接口各自 fail-soft：任一不可用不应把整页打成空白（仍要展示已知部分）
    // 第 1 期冗余收口：Server 存活列表已并入 `/api/north/status` 的 `servers` 字段。
    const [res, pt] = await Promise.all([
      getNorthStatus().catch(() => null),
      getPassthroughSessions().catch(() => null),
    ])
    if (res) {
      status.channels = res.channels || []
      status.aggregated = res.aggregated
      servers.value = res.servers || []
    }
    if (pt) {
      passthrough.enabled = !!pt.enabled
      passthrough.sessions = pt.sessions || []
    }
    lastUpdate.value = formatClock()
  } catch (e) {
    showToast(e, '加载上行服务状态失败')
    logError('uplink-north', e)
  } finally {
    loading.value = false
  }
}

// 自动刷新轮询：usePolling 统一提供隐藏暂停 / 失败退避 / 卸载清理
const { start: startPoll, stop: stopPoll } = usePolling(load, 10000, { autoStart: false })

function toggleAuto(val: boolean) {
  if (val) {
    startPoll()
    // W81: 开启后立即加载一次，避免用户等待 10 秒才能看到首批数据。
    load()
  } else {
    stopPoll()
  }
}

onMounted(load)

onUnmounted(() => {
  // B5: 离开页面时同步重置 autoRefresh=false，避免下次进入页面 autoRefresh=true
  // 但轮询已停止的不一致状态（toggleAuto(true) 才会重新启动）。
  autoRefresh.value = false
})
</script>

<style scoped>
.north-status { padding: 0px; }
.top-bar { display: flex; gap: 16px; align-items: center; margin-bottom: 16px; flex-wrap: wrap; }
.last-update { font-size: 0.75rem; color: var(--text-secondary); }
.mb-16 { margin-bottom: 16px; }

/* C2 统一结论：标题一行说结论，问题清单可逐条对照现场 */
.health-issues {
  margin: 6px 0 0;
  padding-left: 18px;
  font-size: 0.8125rem;
  line-height: 1.7;
}
.health-issues .issue-error { color: var(--text-danger); }
.health-issues .issue-warning { color: var(--text-warning); }

.sk-health { margin-bottom: 16px; }
.sk-health-box { height: 56px; border-radius: var(--radius-table); }
.channel-card {
  border-radius: var(--radius-card); margin-bottom: 16px;
  border: 1px solid var(--border-base);
}
.channel-head {
  display: flex; justify-content: space-between; align-items: center;
  margin-bottom: 12px;
}
.head-left { display: flex; align-items: center; gap: 8px; }
.channel-name { font-size: 0.9375rem; font-weight: 600; color: var(--text-primary); }
.channel-body { display: flex; flex-direction: column; gap: 8px; }
.channel-meta { display: flex; justify-content: space-between; align-items: center; font-size: 0.8125rem; }
.meta-label { color: var(--text-secondary); }
.meta-value { color: var(--text-regular); font-weight: 500; }
.meta-value.broker { font-family: monospace; font-size: 0.75rem; }
.queue-warn { color: var(--text-warning); font-weight: 700; }
.ok { color: var(--text-success); }
.fail { color: var(--text-danger); }
.last-error {
  color: var(--text-danger); font-size: 0.75rem;
  max-width: 60%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
}

/* 骨架占位样式：与真实 channel-card / StatCard 等宽等高 */
.sk-agg { margin-bottom: 16px; }
.sk-stat-box { height: 72px; border-radius: var(--radius-table); }
.sk-head-line { width: 60%; height: 16px; margin-bottom: 12px; }
.sk-body-line { width: 85%; height: 13px; margin-bottom: 8px; }
.sk-body-line.short { width: 50%; margin-bottom: 0; }
</style>
