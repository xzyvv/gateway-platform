<template>
  <!--
    已连接时完全不渲染，避免顶栏冗余。健康徽章已覆盖「全部正常」信号。
    且要求有活跃实时流（activeStreams 非空）：页面无实时功能时不渲染，
    否则运行总览/设备管理等页会常驻误导性的「WS · 已断开」。
  -->
  <!--
    常驻 live region（无障碍 #4b）：徽章是 v-if 条件渲染，只在断开/重连时出现，
    读屏器对「后插入的 live region」播报不可靠，且连接恢复时徽章卸载、
    恢复事件无处播报。这个 sr-only 区域始终在 DOM 中，只更新文案。
  -->
  <span class="sr-only" role="status" :aria-live="announcementUrgency" aria-atomic="true">
    {{ announcement }}
  </span>

  <template v-if="hasLiveStream && (isReconnecting || status === 'disconnected')">
    <el-dropdown trigger="click" class="ws-badge-dropdown">
      <span class="badge-with-actions" :title="tooltipText">
        <div class="health-badge" :class="badgeClass">
          <!-- 状态图标替代圆点（顶栏健康徽章已带红点，双圆点冗余且观感差）：
               重连中 = 旋转 Refresh（通用「处理中」动效），断开 = 静态链环 -->
          <el-icon class="ws-state-ico" :class="{ spinning: isReconnecting }" aria-hidden="true">
            <Refresh v-if="isReconnecting" />
            <Link v-else />
          </el-icon>
          <span>{{ badgeText }}</span>
        </div>
        <el-icon class="triage-caret" v-if="hasHistory" aria-hidden="true"><ArrowDown /></el-icon>
      </span>

      <template #dropdown>
        <el-dropdown-menu class="ws-badge-menu">
          <!-- 头部：当前状态 + 最近错误 -->
          <div class="ws-badge-header">
            <div class="ws-badge-status-row">
              <span class="ws-badge-status-dot" :class="dotClass" aria-hidden="true"></span>
              <span class="ws-badge-status-label">{{ badgeText }}</span>
            </div>
            <div v-if="lastError" class="ws-badge-error">
              <el-icon aria-hidden="true"><Warning /></el-icon>
              <span>{{ lastError }}</span>
            </div>
          </div>

          <!-- 历史事件列表 -->
          <template v-if="hasHistory">
            <el-dropdown-item disabled class="ws-badge-section-title">
              重连历史（最近 {{ reconnectHistory.length }} / 10）
            </el-dropdown-item>
            <el-dropdown-item
              v-for="evt in reconnectHistory"
              :key="evt.time"
              class="ws-history-item"
              :class="'ws-history-' + evt.kind"
            >
              <span class="ws-history-icon">
                <el-icon v-if="evt.kind === 'error'"><CircleCloseFilled /></el-icon>
                <el-icon v-else-if="evt.kind === 'reconnecting'"><Refresh /></el-icon>
                <el-icon v-else><CircleCheckFilled /></el-icon>
              </span>
              <span class="ws-history-text">
                <span v-if="evt.kind === 'error'" class="ws-history-msg">{{ truncate(evt.message, 48) }}</span>
                <span v-else-if="evt.kind === 'reconnecting'">第 {{ evt.attempt }} 次重连调度</span>
                <span v-else>连接已恢复 ✓</span>
              </span>
              <span class="ws-history-time">{{ relativeTime(evt.time) }}</span>
            </el-dropdown-item>

            <el-dropdown-item divided class="ws-badge-actions">
              <div class="ws-badge-action-row">
                <el-button text size="small" type="primary" @click="onReconnectNow">
                  <el-icon><Refresh /></el-icon> 立即重连
                </el-button>
                <el-button text size="small" @click="onClearHistory">
                  清除历史
                </el-button>
              </div>
            </el-dropdown-item>
          </template>

          <template v-else>
            <el-dropdown-item disabled>暂无重连历史</el-dropdown-item>
            <el-dropdown-item divided class="ws-badge-actions">
              <el-button text size="small" type="primary" @click="onReconnectNow">
                <el-icon><Refresh /></el-icon> 立即重连
              </el-button>
            </el-dropdown-item>
          </template>
        </el-dropdown-menu>
      </template>
    </el-dropdown>
  </template>
</template>

<script setup lang="ts">
/**
 * Topbar WS 连接状态徽章
 *
 * 三态视觉：
 *   - 已连接：不渲染（健康徽章已覆盖）
 *   - 重连中：黄色徽章 "第 N 次重连"
 *   - 断开：红色徽章 "已断开"
 *
 * 点击下拉展示：最近 10 条重连历史 + 手动重连按钮
 * （手动重连调用 forceReconnectAll()，对已在连接中的实例幂等跳过）。
 */
import { computed, ref, watch } from 'vue'
import { ArrowDown, Warning, CircleCloseFilled, Refresh, CircleCheckFilled, Link } from '@element-plus/icons-vue'
import { storeToRefs } from 'pinia'
import { useWebsocketStore } from '@/stores/websocket'
import { forceReconnectAll } from '@/utils/websocket'
import { ElMessage } from 'element-plus'

const wsStore = useWebsocketStore()
const { status, reconnectCount, lastError, reconnectHistory, isReconnecting, activeStreams } = storeToRefs(wsStore)

const hasHistory = computed(() => reconnectHistory.value.length > 0)
/** 有实时流实例在跑（如实时数据/性能监控页挂着订阅）才提示断开/重连 */
const hasLiveStream = computed(() => activeStreams.value.size > 0)

/**
 * 无障碍播报（#4b）。
 *
 * 徽章本身是 v-if 条件渲染（只在断开/重连时出现），有两个问题：
 *   1. 读屏器只在「live region 已在 DOM 里、内容随后变化」时才播报。
 *      徽章是断连后才插入的，多数读屏器对「后插入的 live region」播报不可靠；
 *   2. 连接恢复时徽章整体卸载，恢复事件根本无处播报——用户不知道数据已恢复。
 *
 * 因此另设一个常驻的 sr-only live region：它始终在 DOM 中，只把播报文案
 * 写进去。断连用 assertive（现场必须立刻知道），恢复与重连进度用 polite
 * （渐进信息，排队播报不打断当前朗读）。
 */
const announcement = ref('')
const announcementUrgency = ref<'assertive' | 'polite'>('assertive')

/** 记录上一次的状态，避免重复播报同一事件 */
let lastAnnouncedStatus: string | null = null

/**
 * live region 只有「文本内容发生变化」才会触发播报。
 * 两次断连的文案完全相同（"WebSocket 连接已断开"），第二次读屏器会因内容
 * 未变而不播报——用户会以为漏掉了告警。这里在文案尾部追加一个自增计数，
 * 保证每次事件都有新文本，播报必然触发。
 *
 * 计数放在末尾（"WebSocket 连接已断开 (2)"）而不是开头，避免读屏器
 * 先念一个无意义的数字再念正文。
 */
let announceSeq = 0

function announce(text: string, urgency: 'assertive' | 'polite') {
  announceSeq++
  announcementUrgency.value = urgency
  announcement.value = `${text} (${announceSeq})`
}

watch(
  [() => status.value, () => reconnectCount.value],
  ([s, count]) => {
    // 无实时流时不播报：页面本就没实时功能，提示断连是噪音
    if (!hasLiveStream.value) {
      lastAnnouncedStatus = s
      return
    }
    if (s === 'disconnected') {
      // 断连：立即打断播报。同一状态不重复播报（避免每次 reactive 触发都念一遍）
      if (lastAnnouncedStatus === 'disconnected') return
      announce(
        lastError.value ? `WebSocket 连接已断开：${lastError.value}` : 'WebSocket 连接已断开',
        'assertive',
      )
    } else if (s === 'connecting' && count > 0) {
      // 重连进度：polite 排队，不打断
      announce(`WebSocket 正在重连，第 ${count} 次尝试`, 'polite')
    } else if (s === 'connected') {
      // 恢复：徽章已卸载，这里补播恢复事件
      if (lastAnnouncedStatus === 'connected') return
      announce('WebSocket 连接已恢复', 'polite')
    }
    lastAnnouncedStatus = s
  },
  { immediate: true },
)

const badgeClass = computed(() => ({
  'reconnecting': isReconnecting.value,
  'disconnected': status.value === 'disconnected',
}))

const dotClass = computed(() => ({
  'dot-warn': isReconnecting.value,
  'dot-error': status.value === 'disconnected',
}))

const badgeText = computed(() => {
  if (isReconnecting.value) return `WS · 第 ${reconnectCount.value} 次重连`
  return 'WS · 已断开'
})

const tooltipText = computed(() => {
  if (isReconnecting.value) {
    return `WebSocket 连接已断开，正在第 ${reconnectCount.value} 次尝试重连...\n` +
      (lastError.value ? `上次错误：${lastError.value}` : '') +
      '\n点击查看重连历史'
  }
  return `WebSocket 连接已断开` + (lastError.value ? `：${lastError.value}` : '') + '\n点击查看重连历史或手动重连'
})

function relativeTime(ts: number): string {
  const diff = Math.floor((Date.now() - ts) / 1000)
  if (diff < 5) return '刚刚'
  if (diff < 60) return `${diff}秒前`
  if (diff < 3600) return `${Math.floor(diff / 60)}分钟前`
  return `${Math.floor(diff / 3600)}小时前`
}

function truncate(s: string | undefined, n: number): string {
  if (!s) return ''
  return s.length > n ? s.slice(0, n) + '...' : s
}

function onReconnectNow() {
  const n = forceReconnectAll()
  ElMessage.success(n > 0 ? `已触发 ${n} 条 WebSocket 重连` : '当前无活跃 WebSocket 连接')
}

function onClearHistory() {
  wsStore.clearHistory()
  ElMessage.info('重连历史已清除')
}
</script>

<style scoped>
/* 复用健康徽章的视觉语言：同尺寸、同圆点动画、同按钮风格 */
.health-badge {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  padding: 4px 10px;
  border-radius: 16px;
  border: 1px solid var(--border-base);
  font-size: 0.8125rem;
  font-weight: 500;
  cursor: pointer;
  transition: background 0.2s, border-color 0.2s;
  user-select: none;
}
.health-badge:hover {
  opacity: 0.9;
}

.health-badge.reconnecting {
  background: var(--color-warning-bg);
  color: var(--text-warning);
  border-color: var(--color-warning-border);
}
.health-badge.disconnected {
  background: var(--color-danger-bg);
  color: var(--text-danger);
  border-color: var(--color-danger-border);
}

/* 状态图标：重连中旋转（通用「处理中」动效），断开静态链环 */
.ws-state-ico {
  font-size: 0.875em;
}
.ws-state-ico.spinning {
  animation: wsSpin 1.2s linear infinite;
}
@keyframes wsSpin {
  to { transform: rotate(360deg); }
}

.badge-with-actions {
  display: inline-flex;
  align-items: center;
  gap: 2px;
  cursor: pointer;
}
.triage-caret {
  font-size: 0.6875rem;
  color: var(--text-secondary);
}

/* ===== 下拉菜单 ===== */
.ws-badge-menu {
  min-width: 320px;
}

.ws-badge-header {
  padding: 12px 16px 8px;
  border-bottom: 1px solid var(--border-extralight);
  margin-bottom: 4px;
}
.ws-badge-status-row {
  display: flex;
  align-items: center;
  gap: 8px;
  font-weight: 600;
  font-size: 0.875rem;
  color: var(--text-primary);
}
.ws-badge-status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--color-warning);
}
.ws-badge-status-dot.dot-error {
  background: var(--color-danger);
}
.ws-badge-status-dot.dot-warn {
  background: var(--color-warning);
}
.ws-badge-error {
  display: flex;
  gap: 6px;
  margin-top: 6px;
  font-size: 0.75rem;
  color: var(--text-danger);
  align-items: flex-start;
  line-height: 1.4;
  word-break: break-all;
}

.ws-badge-section-title {
  font-size: 0.6875rem;
  font-weight: 600;
  color: var(--text-secondary);
  padding: 8px 16px 4px;
}

.ws-history-item {
  display: flex !important;
  align-items: center;
  gap: 8px;
  padding: 6px 16px;
  font-size: 0.8125rem;
  line-height: 1.4;
}
.ws-history-icon {
  flex-shrink: 0;
  font-size: 0.875rem;
  color: var(--text-secondary);
}
.ws-history-error .ws-history-icon {
  color: var(--color-danger);
}
.ws-history-reconnecting .ws-history-icon {
  color: var(--color-warning);
}
.ws-history-recovered .ws-history-icon {
  color: var(--color-success);
}
.ws-history-text {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.ws-history-error .ws-history-text {
  color: var(--text-regular);
}
.ws-history-recovered .ws-history-text {
  color: var(--text-success);
  font-weight: 500;
}
.ws-history-time {
  flex-shrink: 0;
  font-size: 0.6875rem;
  color: var(--text-placeholder);
}

.ws-badge-actions {
  padding: 4px 8px;
}
.ws-badge-action-row {
  display: flex;
  gap: 4px;
}

/* 中等屏幕下隐藏文字只留圆点 */
@media (max-width: 1200px) and (min-width: 769px) {
  .health-badge {
    padding: 4px;
  }
  .health-badge > span:last-child {
    display: none;
  }
}
</style>
