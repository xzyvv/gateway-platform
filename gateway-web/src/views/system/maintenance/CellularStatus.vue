<template>
  <div class="cellular-status">
    <!-- W99: 与 VISUAL_SPEC §8.2 一致——使用 card-header + section-header::before 渐变装饰 -->
    <SectionHeader title="蜂窝网络" icon="Promotion">
      <template #extra>
        <el-tooltip v-if="data" content="仅表示配置项已开启，实际运行状态见下方连接状态" placement="top">
          <el-tag size="small" :type="data.enabled ? 'success' : 'info'" effect="plain" round class="count-badge">
            {{ data.enabled ? '配置已启用' : '配置未启用' }}
          </el-tag>
        </el-tooltip>
      </template>
      <el-button type="primary" size="small" @click="load" :loading="loading">
      <el-icon><Refresh /></el-icon> 刷新
      </el-button>
      <el-tooltip content="每 10 秒自动刷新" placement="top">
      <el-switch v-model="autoRefresh" active-text="自动刷新" @change="toggleAuto" />
      </el-tooltip>
    </SectionHeader>
    <span v-if="lastUpdate" class="last-update">最后更新：{{ lastUpdate }}</span>

    <!-- B: 平台能力诚实化提示——非 Linux 平台不发起 4G 拨号，避免用户误以为蜂窝已可上网 -->
    <el-alert
      v-if="data && data.enabled && data.dial_supported === false"
      type="warning"
      show-icon
      :closable="false"
      class="platform-alert"
      title="当前平台不支持 4G 拨号"
      :description="`本网关运行在 ${data.platform}，蜂窝模块仅执行链路监控与故障切换，不会发起 PPP 拨号。`"
    />

    <div>
      <!-- 首屏骨架：与真实内容同构（6 张信息卡 + 链路卡片 + 信号卡片），数据到达时布局不跳 -->
      <template v-if="loading && !data">
        <el-row :gutter="16">
          <el-col :xs="24" :sm="12" :md="8" v-for="i in 6" :key="`sk-info-${i}`">
            <el-card shadow="never" class="info-card">
              <el-skeleton animated>
                <template #template>
                  <el-skeleton-item variant="text" class="sk-label" />
                  <el-skeleton-item variant="text" class="sk-value" />
                </template>
              </el-skeleton>
            </el-card>
          </el-col>
        </el-row>

        <el-card shadow="never" class="link-card">
          <el-skeleton animated>
            <template #template>
              <div class="sk-row">
                <el-skeleton-item variant="text" class="sk-label" />
                <el-skeleton-item variant="text" class="sk-tag" />
              </div>
            </template>
          </el-skeleton>
        </el-card>

        <el-card shadow="never" class="signal-card">
          <el-skeleton animated>
            <template #template>
              <div class="sk-row">
                <el-skeleton-item variant="text" class="sk-label" />
                <el-skeleton-item variant="text" class="sk-value-short" />
              </div>
              <el-skeleton-item variant="rect" class="sk-bar" />
            </template>
          </el-skeleton>
        </el-card>
      </template>

      <template v-else>
        <empty-state v-if="!data || !data.enabled" description="蜂窝模块未启用" />

        <template v-else-if="data && data.status">
          <el-row :gutter="16">
            <el-col :xs="24" :sm="12" :md="8" v-for="item in statusItems" :key="item.label">
              <el-card shadow="never" class="info-card">
                <div class="info-label">{{ item.label }}</div>
                <div class="info-value" :style="{ color: item.color || 'var(--text-primary)' }">
                  {{ item.value }}
                </div>
              </el-card>
            </el-col>
          </el-row>

          <el-card shadow="never" class="link-card" v-if="data.link_state">
            <div class="link-row">
              <span class="link-label">链路状态</span>
              <el-tag :type="linkTagType" effect="dark" round>{{ linkStateText }}</el-tag>
            </div>
          </el-card>

          <el-card shadow="never" class="signal-card" v-if="data.status.signal_strength != null">
            <div class="signal-head">
              <span class="signal-label">信号强度</span>
              <span class="signal-value">{{ signalKnown ? `${data.status.signal_strength} / 31` : '未知' }}</span>
            </div>
            <el-progress
              :percentage="signalPercent"
              :color="signalColor"
              :stroke-width="10"
              :show-text="false"
            />
          </el-card>
        </template>

        <empty-state v-else-if="data && data.enabled && !data.status" description="蜂窝模块已启用，暂无状态数据" />
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import {Refresh} from '@element-plus/icons-vue'
import { getCellularStatus, type CellularStatus } from '@/api/network'
import { extractErrorMessage } from '@/utils/errorHandler'
import { logError } from '@/utils/logger'
import { usePolling } from '@/composables/core/usePolling'
import { formatClock, formatDateTime } from '@/utils/datetime'
import { statusTagType, type TagType } from '@/utils/statusType'

const data = ref<CellularStatus | null>(null)
const loading = ref(false)
const autoRefresh = ref(false)
const lastUpdate = ref('')

// 信号强度使用 AT+CSQ 的原始 RSSI：有效区间 0-31，99 表示未知/无信号（并非 dBm）
const signalKnown = computed(() => {
  const v = data.value?.status?.signal_strength
  return v != null && v >= 0 && v <= 31
})

// C: 后端 CellularState 枚举 → 中文（后端保持英文枚举，本地化在前端）
const stateText = computed(() => {
  const map: Record<string, string> = {
    Init: '初始化中',
    NotReady: '未就绪',
    SimNotReady: 'SIM 未就绪',
    Registered: '已注册',
    Dialing: '拨号中',
    Connected: '已连接',
    DialFailed: '拨号失败',
  }
  const s = data.value?.status?.state
  return s ? (map[s] ?? s) : '-'
})

const statusItems = computed(() => {
  if (!data.value || !data.value.status) return []
  const s = data.value.status
  return [
    { label: '连接状态', value: stateText.value, color: 'var(--color-primary)' },
    { label: '运营商', value: s.operator || '-' },
    { label: 'IP 地址', value: s.ip_address || '-' },
    { label: '网络类型', value: s.network_type || '-' },
    { label: '信号强度', value: signalKnown.value ? `CSQ ${s.signal_strength}` : '未知' },
    { label: '更新时间', value: formatTime(s.updated_at) },
  ]
})

/**
 * 链路状态 → 徽章颜色。
 * Down 已在通用表（down=danger）；CellularUp 是本页特有词，本地登记为 warning；
 * 其余（EthernetUp / BothUp / 未知）按原语义落 success。
 */
const LINK_TAG_OVERRIDE: Record<string, TagType> = {
  cellularup: 'warning',
}
const linkTagType = computed<TagType>(() => {
  const state = data.value?.link_state
  const key = String(state ?? '').trim().toLowerCase()
  const hit = LINK_TAG_OVERRIDE[key]
  if (hit) return hit
  const t = statusTagType(state)
  return t === 'info' ? 'success' : t
})

const linkStateText = computed(() => {
  const map: Record<string, string> = {
    EthernetUp: '以太网上行',
    CellularUp: '蜂窝上行',
    BothUp: '双通道上行',
    Down: '链路断开',
  }
  return data.value?.link_state ? (map[data.value.link_state!] || data.value.link_state!) : '-'
})

const signalPercent = computed(() => {
  // CSQ(0-31) → 百分比；未知(99)或越界返回 0，避免无信号时误显示满格
  const v = data.value?.status?.signal_strength
  if (v == null || v < 0 || v > 31) return 0
  return Math.round((v / 31) * 100)
})

const signalColor = computed(() => {
  const pct = signalPercent.value
  if (pct >= 66) return 'var(--text-success)'
  if (pct >= 33) return 'var(--text-warning)'
  return 'var(--text-danger)'
})

function formatTime(t: number) {
  if (!t) return '-'
  const ms = t > 1e12 ? t : t * 1000
  return formatDateTime(ms)
}

async function load() {
  loading.value = true
  try {
    data.value = await getCellularStatus()
    lastUpdate.value = formatClock()
  } catch (e) {
    // W102: 提取具体错误信息便于排查——此前仅显示"加载失败"，运维无法定位
    const detail = extractErrorMessage(e, '')
    ElMessage.error(`加载蜂窝网络状态失败：${detail || '未知错误'}`)
    logError('cell', e)
  } finally {
    loading.value = false
  }
}

// 自动刷新统一走 usePolling：开关式（autoStart:false）、标签页隐藏暂停、卸载自动清理
const { start: startPoll, stop: stopPoll } = usePolling(load, 10000, { autoStart: false })

function toggleAuto(val: boolean) {
  // W100: start(true) 开启后立即加载一次，避免用户等待 10 秒才能看到首批数据
  if (val) startPoll(true)
  else stopPoll()
}

onMounted(load)
</script>

<style scoped>
.cellular-status { padding: 0px; }
.top-bar { display: flex; gap: 16px; align-items: center; margin-bottom: 16px; flex-wrap: wrap; }
.last-update { font-size: 0.75rem; color: var(--text-secondary); }
.platform-alert { margin-bottom: 16px; }
.info-card { border-radius: var(--radius-card); margin-bottom: 16px; border: 1px solid var(--border-base); }
.info-label { font-size: 0.75rem; color: var(--text-secondary); margin-bottom: 6px; }
.info-value { font-size: 1rem; font-weight: 600; word-break: break-all; }
.link-card { border-radius: var(--radius-card); margin-bottom: 16px; border: 1px solid var(--border-base); }
.link-row { display: flex; justify-content: space-between; align-items: center; }
.link-label { font-size: 0.875rem; color: var(--text-secondary); }
.signal-card { border-radius: var(--radius-card); border: 1px solid var(--border-base); }
.signal-head { display: flex; justify-content: space-between; align-items: center; margin-bottom: 12px; }
.signal-label { font-size: 0.875rem; color: var(--text-secondary); }
.signal-value { font-size: 1.125rem; font-weight: 700; color: var(--text-primary); }

/* 首屏骨架占位：与真实 info-card / link-card / signal-card 等宽等高 */
.sk-row { display: flex; justify-content: space-between; align-items: center; }
.sk-row .sk-label { margin-bottom: 0; }
.sk-label { width: 40%; height: 12px; margin-bottom: 6px; }
.sk-value { width: 70%; height: 16px; }
.sk-tag { width: 72px; height: 22px; border-radius: 999px; }
.sk-value-short { width: 56px; height: 18px; }
.sk-bar { height: 10px; border-radius: 5px; margin-top: 12px; }
</style>
