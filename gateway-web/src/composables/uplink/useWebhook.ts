import { ref, watch } from 'vue'
import { isTauri, forwardWebhook } from '@/api'
import { extractErrorMessage } from '@/utils/errorHandler'
import { scopedGet, scopedSet } from '@/utils/gatewayStorage'
import {
  formatUrlSafetyError,
  validateOutboundUrl,
} from '@/utils/urlSafety'

/**
 * 变更通知 / Webhook — 带重试 + 通知重试队列（本机浏览器）
 * - 发送失败按指数退避自动重试（3 次）
 * - 仍失败的通知进入「通知重试队列」，可手动重发
 * - URL 与通知重试队列均持久化到 localStorage
 *
 * 注意：本文件里的队列是**本机浏览器缓存**，只记录「这台电脑发出的通知」。
 * 它**不是设备端数据死信**：不进网关数据库、不随设备迁移、换台电脑就是空的。
 * 设备端未送达的数据请走 `@/api/deadLetter`（GET /api/north/dead-letters）。
 */
const STORAGE_KEY = 'gw_webhook_url'
const NOTIFY_QUEUE_KEY = 'gw_webhook_notify_queue'

export interface WebhookEvent {
  event: string
  message: string
  timestamp: number
  data?: Record<string, unknown>
}

interface QueuedEvent {
  id: string
  payload: WebhookEvent
  retryCount: number
  lastError?: string
}

const MAX_RETRY = 3
const MAX_NOTIFY_QUEUE = 50

function readUrl(): string {
  try { return scopedGet(STORAGE_KEY) || '' } catch { return '' }
}

function readNotifyQueue(): QueuedEvent[] {
  try {
    const raw = scopedGet(NOTIFY_QUEUE_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    // 过滤脏数据：缺 id 或 payload 一律丢弃，避免 retryNotify 找不到/出错
    return parsed.filter((q: unknown): q is QueuedEvent =>
      !!q && typeof q === 'object' && typeof (q as QueuedEvent).id === 'string' &&
      !!(q as QueuedEvent).payload && typeof (q as QueuedEvent).payload === 'object'
    ).slice(-MAX_NOTIFY_QUEUE)
  } catch { return [] }
}

// @singleton: Webhook 配置需跨页面共享（设置页 + 通知触发点），有意使用模块级 ref
// 持久化到 localStorage，后续如需 devtools 追踪可迁移到 Pinia store
// eslint-disable-next-line no-restricted-syntax
const webhookUrl = ref(readUrl())
// eslint-disable-next-line no-restricted-syntax
const notifyRetryQueue = ref<QueuedEvent[]>(readNotifyQueue())

watch(webhookUrl, (v) => {
  try { scopedSet(STORAGE_KEY, v) } catch { /* ignore */ }
})
watch(notifyRetryQueue, (v) => {
  try { scopedSet(NOTIFY_QUEUE_KEY, JSON.stringify(v)) } catch { /* ignore */ }
}, { deep: true })

function delay(ms: number) {
  return new Promise((r) => setTimeout(r, ms))
}

export function useWebhook() {
  /**
   * W-14 修复：设置 webhook URL 时同步做基本安全校验。
   * 失败抛错给 UI 层（设置页 catch 后显示错误消息）。
   * 同步校验只覆盖 scheme + host 字面黑名单（不查 DNS，避免 UI 卡顿）。
   * 真正发送时（forwardWebhook / fetch）会再做一次同样的静态校验。
   */
  function setUrl(url: string) {
    const trimmed = url.trim()
    if (!trimmed) {
      webhookUrl.value = ''
      return
    }
    const result = validateOutboundUrl(trimmed)
    if (!result.ok) {
      throw new Error(formatUrlSafetyError(result.error))
    }
    webhookUrl.value = trimmed
  }

  function isEnabled(): boolean {
    return !!webhookUrl.value
  }

  async function sendOnce(payload: WebhookEvent): Promise<void> {
    if (!webhookUrl.value) return
    // 防御：localStorage 可能被外部修改（如恶意扩展），保存时校验过
    // 但读出时仍可能含过期配置。sendOnce 入口再做一次同步校验防绕过。
    const guard = validateOutboundUrl(webhookUrl.value)
    if (!guard.ok) {
      throw new Error(`webhook url blocked: ${formatUrlSafetyError(guard.error)}`)
    }
    // Tauri 模式：通过 Rust 转发，避免 webview 直连外部 HTTP（CSP 收紧后必需）
    // forwardWebhook 内部还会再校验一次，并 invoke 到 Rust 端（Rust 端出网前强制校验）
    if (isTauri()) {
      await forwardWebhook(webhookUrl.value, JSON.stringify(payload))
      return
    }
    // Web 模式：fetch 走浏览器同源策略——但 file:// 等 scheme 仍被浏览器
    // 拦截；http/https 跨源 POST 会被预检挡住。这里仍做一次校验作 defense-in-depth。
    await fetch(webhookUrl.value, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    })
  }

  /** 发送通知，失败自动重试，全部失败后进入本机通知重试队列 */
  async function notify(event: string, message: string, data?: Record<string, unknown>): Promise<void> {
    if (!webhookUrl.value) return
    const payload: WebhookEvent = { event, message, timestamp: Date.now(), ...(data ? { data } : {}) }
    const id = `${event}-${payload.timestamp}-${Math.random().toString(36).slice(2, 7)}`
    let lastErr = ''
    for (let attempt = 0; attempt <= MAX_RETRY; attempt++) {
      try {
        await sendOnce(payload)
        return // 成功
      } catch (e) {
        lastErr = extractErrorMessage(e)
        if (attempt < MAX_RETRY) {
          // 指数退避：1s, 2s, 4s
          await delay(1000 * 2 ** attempt)
        }
      }
    }
    // 入通知重试队列（截断到 MAX_NOTIFY_QUEUE，丢弃最旧的避免撑爆 localStorage）
    notifyRetryQueue.value.push({ id, payload, retryCount: MAX_RETRY, lastError: lastErr })
    if (notifyRetryQueue.value.length > MAX_NOTIFY_QUEUE) {
      notifyRetryQueue.value.splice(0, notifyRetryQueue.value.length - MAX_NOTIFY_QUEUE)
    }
  }

  /** 重发单个通知 */
  async function retryNotify(id: string): Promise<boolean> {
    const idx = notifyRetryQueue.value.findIndex((q) => q.id === id)
    if (idx < 0) return false
    const item = notifyRetryQueue.value[idx]!
    try {
      await sendOnce(item.payload)
      notifyRetryQueue.value.splice(idx, 1)
      return true
    } catch (e) {
      item.retryCount++
      item.lastError = extractErrorMessage(e)
      return false
    }
  }

  /** 重发全部通知 */
  async function retryAllNotify(): Promise<{ ok: number; fail: number }> {
    let ok = 0
    let fail = 0
    for (const item of [...notifyRetryQueue.value]) {
      if (await retryNotify(item.id)) ok++
      else fail++
    }
    return { ok, fail }
  }

  /** 清空通知重试队列（仅本机浏览器，不影响设备端死信） */
  function clearNotifyQueue() {
    notifyRetryQueue.value = []
  }

  return {
    // state
    webhookUrl,
    notifyRetryQueue,
    // methods
    setUrl,
    isEnabled,
    notify,
    retryNotify,
    retryAllNotify,
    clearNotifyQueue,
  }
}
