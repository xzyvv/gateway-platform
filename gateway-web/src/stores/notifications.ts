import { defineStore } from 'pinia'
import { h, ref, computed } from 'vue'
import { ElMessage, ElNotification } from 'element-plus'

export type NotificationType = 'success' | 'error' | 'warning' | 'info'

export interface AppNotification {
  id: number
  type: NotificationType
  title: string
  message: string
  code?: number
  /** 后端 trace ID（响应头 x-request-id）：通知中心可复制，研发凭此查网关日志 */
  traceId?: string
  timestamp: number
  read: boolean
  /** 发生该通知时的页面路径（含 query），供通知中心一键跳回现场 */
  from?: string
}

/**
 * 集中通知 Store
 *
 * 统一管理全局通知，替代各页面直接调用 ElMessage。
 * - 通知队列（最近 50 条，供通知中心展示）
 * - 自动弹出（Toast / Notification）
 * - 错误去重（同文案滑动窗口 3s 抑制）+ 弹窗节流（错误/警告最多 3s 一次）
 * - 错误码透传（来自后端 ApiError.code）
 */
export const useNotificationsStore = defineStore('notifications', () => {
  const items = ref<AppNotification[]>([])
  const MAX_ITEMS = 50
  const DEDUP_WINDOW_MS = 3000
  /** 同类弹窗的最小间隔：错误/警告最多 3s 弹一次 */
  const POPUP_MIN_INTERVAL_MS = 3000

  let seq = 0
  /** 各类型最近一次真正弹窗的时刻，用于全局节流 */
  const lastPopupAt: Record<NotificationType, number> = {
    success: 0, error: 0, warning: 0, info: 0,
  }

  const unreadCount = computed(() => items.value.filter((n) => !n.read).length)

  function push(
    type: NotificationType,
    message: string,
    opts: { title?: string; code?: number; silent?: boolean; traceId?: string } = {},
  ): AppNotification {
    const now = Date.now()
    // 记录当前页面路径：通知中心可一键跳回"出错现场"（写操作失败排障最常用）。
    // 路由为 history 模式，pathname+search 即完整地址；不 import router，
    // 避免 store → router → views → store 的循环依赖。
    const from =
      typeof window !== 'undefined'
        ? window.location.pathname + window.location.search
        : undefined
    const recent = items.value.find(
      (n) => n.type === type && n.message === message && now - n.timestamp < DEDUP_WINDOW_MS,
    )
    if (recent) {
      // 滑动窗口去重：持续重复的同一条错误（典型场景是网关失联时每 3s 一次的
      // 轮询失败）只刷新这条记录的时间戳并置顶，不新建条目、也不再弹窗。
      //
      // 旧实现从「首次出现时刻」起算固定窗口，同一条错误每满 3 秒就会再弹一次，
      // 断网期间屏幕上会持续堆叠相同的提示。失联这种"持续状态"已经由
      // ConnectivityBanner + 顶栏健康徽章常驻展示，不需要靠重复弹窗提醒；
      // 去重改为滑动窗口后，错误停止 3 秒以上再出现才会重新提醒。
      recent.timestamp = now
      const idx = items.value.indexOf(recent)
      if (idx > 0) {
        items.value.splice(idx, 1)
        items.value.unshift(recent)
      }
      return recent
    }

    const item: AppNotification = {
      id: ++seq,
      type,
      title: opts.title ?? defaultTitle(type),
      message,
      code: opts.code,
      traceId: opts.traceId,
      timestamp: now,
      read: false,
      from,
    }
    items.value.unshift(item)
    if (items.value.length > MAX_ITEMS) items.value.pop()

    if (!opts.silent) popup(item, now)
    return item
  }

  /**
   * 弹窗节流。
   *
   * 除「同文案去重」外还需要全局节流：网关失联时多个接口会在同一秒内以不同
   * 文案失败（`请求失败(502): /api/devices`、`请求失败(502): /api/points` …），
   * 文案不同就绕过了去重，照样能刷屏。这里限制错误/警告最多每 3 秒弹一次，
   * 被压掉的条目仍完整写入通知中心，不丢信息。
   */
  function popup(item: AppNotification, now: number) {
    const isAlert = item.type === 'error' || item.type === 'warning'
    if (isAlert && now - lastPopupAt[item.type] < POPUP_MIN_INTERVAL_MS) return
    lastPopupAt[item.type] = now

    if (isAlert) {
      // 带 traceId 时把 ID 直接显示在弹窗里：用户截图/复制即可报障，
      // 不必先打开通知中心（错误可定位闭环的关键一步）。
      const message = item.traceId
        ? h('div', [
            h('div', item.message),
            h(
              'div',
              {
                style:
                  'margin-top:4px;font-size:12px;opacity:.75;font-family:Consolas,monospace;word-break:break-all',
              },
              `traceId: ${item.traceId}`,
            ),
          ])
        : item.message
      ElNotification({
        type: item.type,
        title: item.title,
        message,
        duration: item.type === 'error' ? 5000 : 3000,
      })
    } else {
      toast(item.type, item.message)
    }
  }

  function toast(type: NotificationType, message: string) {
    ElMessage({ type, message, showClose: true })
  }

  function defaultTitle(type: NotificationType): string {
    switch (type) {
      case 'success': return '操作成功'
      case 'error': return '操作失败'
      case 'warning': return '注意'
      case 'info': return '提示'
    }
  }

  type PushOpts = { code?: number; silent?: boolean; traceId?: string }

  function success(message: string, opts?: PushOpts) {
    return push('success', message, opts)
  }
  function error(message: string, opts?: PushOpts) {
    return push('error', message, opts)
  }
  function warning(message: string, opts?: PushOpts) {
    return push('warning', message, opts)
  }
  function info(message: string, opts?: PushOpts) {
    return push('info', message, opts)
  }

  function markAllRead() {
    items.value.forEach((n) => (n.read = true))
  }
  function clear() {
    items.value = []
  }
  function remove(id: number) {
    items.value = items.value.filter((n) => n.id !== id)
  }

  return {
    items,
    unreadCount,
    push,
    success,
    error,
    warning,
    info,
    markAllRead,
    clear,
    remove,
  }
})
