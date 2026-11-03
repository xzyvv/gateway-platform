<template>
  <div class="notice-center">
    <el-badge
      :value="unread"
      :max="99"
      :hidden="unread === 0"
      class="notice-badge"
    >
      <el-button
        text
        class="icon-btn"
        :title="unread ? `通知中心（${unread} 条未读）` : '通知中心'"
        :aria-label="unread ? `通知中心，${unread} 条未读` : '通知中心'"
        @click="visible = true"
      >
        <el-icon><Bell /></el-icon>
      </el-button>
    </el-badge>

    <!-- append-to-body 必须开启：顶栏 .header 带 backdrop-filter（glass 效果），
         会为 position: fixed 的子元素创建包含块。抽屉若不 teleport 到 body，
         会被限制在 60px 高的顶栏内，标题「通知中心」被挤压遮挡。 -->
    <el-drawer v-model="visible" title="通知中心" size="420px" append-to-body @open="onOpen">
      <div class="notice-toolbar">
        <span class="notice-hint">写操作失败不再弹窗，只在这里留痕</span>
        <el-button size="small" text :disabled="!store.items.length" @click="store.clear()">
          清空
        </el-button>
      </div>

      <EmptyState v-if="!store.items.length" description="暂无通知 ✨ 写操作失败会在这里留痕" :image-size="60" />

      <ul v-else class="notice-list">
        <li
          v-for="n in store.items"
          :key="n.id"
          class="notice-item"
          :class="[`is-${n.type}`, { 'is-unread': !n.read }]"
        >
          <div class="notice-head">
            <el-tag :type="tagType(n.type)" size="small" effect="plain">{{ n.title }}</el-tag>
            <span class="notice-time">{{ formatTime(n.timestamp) }}</span>
            <el-button
              v-if="n.from"
              text
              size="small"
              class="notice-act notice-go"
              :title="`前往${fromLabel(n.from)}`"
              :aria-label="`前往${fromLabel(n.from)}`"
              @click="goTo(n.from)"
            >
              <el-icon><Position /></el-icon>
            </el-button>
            <el-button
              text
              size="small"
              class="notice-act"
              aria-label="复制这条通知"
              title="复制这条通知"
              @click="copyOne(n)"
            >
              <el-icon><CopyDocument /></el-icon>
            </el-button>
            <el-button
              text
              size="small"
              class="notice-act"
              aria-label="删除这条通知"
              title="删除这条通知"
              @click="store.remove(n.id)"
            >
              <el-icon><Close /></el-icon>
            </el-button>
          </div>
          <p class="notice-msg">{{ n.message }}</p>
          <p v-if="n.code !== undefined" class="notice-code">{{ codeLine(n.code) }}</p>
          <p v-if="n.traceId" class="notice-code notice-trace" :title="`后端 trace ID：${n.traceId}`">
            traceId · {{ n.traceId }}
          </p>
        </li>
      </ul>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
/**
 * 通知中心
 *
 * 背景：全局拦截器对写操作（POST/PUT/DELETE/PATCH）失败只写入 notifications
 * store、不弹 Toast，避免与页面自己的动作语境提示重复。既然不弹窗，就必须有
 * 一个「事后可查」的入口，否则失败对用户完全不可见 —— 本组件就是这个入口。
 *
 * 职责：
 *   - 顶栏铃铛 + 未读角标
 *   - 抽屉按时间倒序列出最近 50 条（store 已限量）
 *   - 打开即全部标记已读；单条可复制/删除；可整体清空
 */
import { computed, ref } from 'vue'
import { useRouter } from 'vue-router'
import { ElBadge } from 'element-plus'
// 徽标样式必须手动带：ElBadge 是静态 import，模板 <el-badge> 被解析成 setup 绑定后
// ElementPlusResolver 看不到它，按需样式不会注入。缺了 el-badge.css 时 .el-badge
// 没有 position:relative，而带默认插槽的角标必然是 is-fixed（position:absolute），
// 于是全站顶栏通知徽标锚点错乱。
import 'element-plus/es/components/badge/style/css'
import { Bell, CopyDocument, Close, Position } from '@element-plus/icons-vue'
import { useNotificationsStore, type AppNotification, type NotificationType } from '@/stores/notifications'
import { describeErrorDetail, isHttpStatusCode } from '@/utils/errorCodes'
import { buildDiagnosticText } from '@/utils/diagnosticReport'
import { copyText } from '@/utils/clipboard'

const store = useNotificationsStore()
const router = useRouter()
const visible = ref(false)
const unread = computed(() => store.unreadCount)

const TAG_TYPE: Record<NotificationType, 'success' | 'warning' | 'danger' | 'info'> = {
  success: 'success',
  warning: 'warning',
  error: 'danger',
  info: 'info',
}

function tagType(t: NotificationType) {
  return TAG_TYPE[t]
}

function onOpen() {
  store.markAllRead()
}

/** 跳回通知发生时的页面。标题取自路由 meta.title，取不到则用兜底文案 */
function fromLabel(from: string): string {
  try {
    const title = router.resolve(from).meta?.title as string | undefined
    return title ? `「${title}」` : '来源页面'
  } catch {
    return '来源页面'
  }
}

function goTo(from: string) {
  visible.value = false
  void router.push(from).catch(() => { /* 目标路由已下线等场景：跳转失败静默 */ })
}

/**
 * 错误码行文案：命中映射表时给「是什么 + 下一步」，未命中保持「错误码 N」。
 * - HTTP 状态码（100–599，如 502）→「HTTP 502 · 网关服务未就绪｜下一步：…」
 * - 已知业务码（8000/4021）→「错误码 8000 · 登录状态已失效｜下一步：…」
 */
function codeLine(code: number): string {
  const detail = describeErrorDetail(code)
  if (!detail) return `错误码 ${code}`
  const head = isHttpStatusCode(code) ? `HTTP ${code}` : `错误码 ${code}`
  return detail.suggestion
    ? `${head} · ${detail.label}｜下一步：${detail.suggestion}`
    : `${head} · ${detail.label}`
}

/** 当天只显示时分秒，跨天补日期，避免"09:12:03"分不清是哪天 */
function formatTime(ts: number): string {
  const d = new Date(ts)
  const pad = (n: number) => String(n).padStart(2, '0')
  const hms = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  const today = new Date()
  const sameDay =
    d.getFullYear() === today.getFullYear() &&
    d.getMonth() === today.getMonth() &&
    d.getDate() === today.getDate()
  return sameDay ? hms : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hms}`
}

/**
 * 复制一条通知：带上 traceId / 错误码 / 来源页面，形成可直接提交的排障凭据。
 * 走 copyText 而非 navigator.clipboard —— 现场常用 http:// 非安全上下文，
 * 后者不可用时会静默失败。
 */
async function copyOne(n: AppNotification) {
  await copyText(
    buildDiagnosticText({
      time: n.timestamp,
      code: n.code,
      traceId: n.traceId,
      error: `${n.title}: ${n.message}`,
      page: n.from,
    }),
    '诊断信息已复制',
  )
}
</script>

<style scoped>
.notice-center {
  display: inline-flex;
  align-items: center;
}

.notice-badge :deep(.el-badge__content) {
  pointer-events: none;
}

.notice-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 12px;
}

.notice-hint {
  font-size: 0.75rem;
  color: var(--el-text-color-secondary);
  line-height: 1.5;
}

.notice-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.notice-item {
  border: 1px solid var(--el-border-color-lighter);
  border-left: 3px solid var(--el-border-color);
  border-radius: var(--radius-button);
  padding: 8px 10px;
  background: var(--el-fill-color-blank);
}

/* 左侧色条按级别区分：错误红 / 警告黄 / 成功绿 / 信息灰 */
.notice-item.is-error { border-left-color: var(--el-color-danger); }
.notice-item.is-warning { border-left-color: var(--el-color-warning); }
.notice-item.is-success { border-left-color: var(--el-color-success); }
.notice-item.is-info { border-left-color: var(--el-color-info); }

.notice-item.is-unread { background: var(--el-fill-color-light); }

.notice-head {
  display: flex;
  align-items: center;
  gap: 6px;
}

.notice-time {
  margin-left: auto;
  font-size: 0.75rem;
  color: var(--el-text-color-secondary);
  font-variant-numeric: tabular-nums;
}

.notice-act {
  padding: 0 2px;
  color: var(--el-text-color-secondary);
}

/* 跳转按钮用主色突出：排障时"回到出错页面"是最高频动作 */
.notice-go {
  color: var(--el-color-primary);
}

.notice-msg {
  margin: 6px 0 0;
  font-size: 0.8125rem;
  line-height: 1.55;
  color: var(--el-text-color-regular);
  word-break: break-word;
}

.notice-code {
  margin: 4px 0 0;
  font-size: 0.75rem;
  /* 建议文案需要可读：占位灰太淡，用次要文字色（厂区强光下也能看清） */
  color: var(--el-text-color-secondary);
}

/* traceId 是排障凭据：等宽 + 单击全选，方便手抄或直接复制 */
.notice-trace {
  font-family: Consolas, monospace;
  color: var(--el-text-color-regular);
  user-select: all;
  word-break: break-all;
}
</style>
