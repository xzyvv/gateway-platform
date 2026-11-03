/**
 * 全局在飞请求注册中心
 *
 * 从 http.ts 抽出，切断 gatewayConfig.ts ↔ http.ts 的循环依赖：
 * gatewayConfig 的 Tauri 适配器被 http.ts 静态引用，而 setActiveGateway
 * 切网关后又需要 abort + reload；此前只能用动态 import('./http') 绕行，
 * 导致 vite 打出 INEFFECTIVE_DYNAMIC_IMPORT 警告（http.ts 已被多处静态引入，
 * 动态导入零拆包收益）。本模块不依赖任何 api 层兄弟模块，可被双方静态引用。
 *
 * Bug 2.7 (2026-Q1) 修复：原实现用裸 Set<AbortController>，无上界。
 * 一旦 addInflight/removeInflight 配对失衡（第三方拦截器/未捕获异常），
 * Set 持续膨胀，内存泄漏且 abortAllInflight 越来越慢。
 * 改为带上限（256）的注册中心，并记录超限丢弃计数供诊断。
 *
 * W-3 修复 (2026-10-21)：原实现在触顶时 abort + 淘汰"最早注册"的 controller。
 * 被淘汰的是一个**完全正常的在飞业务请求**，且它只会走响应拦截器的
 * `axios.isCancel` 分支——不弹提示、不写日志，用户表现为"部分卡片一直不刷新"
 * 的幽灵 bug。现改为触顶时**跳过登记**：该请求照常完成，只是不再受
 * abortAllInflight() 管辖（真正需要强杀的只有切网关/登出，那已由它显式覆盖）。
 */

import { logWarn } from '../utils/logger'

/** 在飞请求登记上限：达到后新请求跳过登记（而不 abort 正常请求） */
const MAX_INFLIGHT = 256
/** 超过此阈值时打印 warn 日志（用于现场诊断） */
const INFLIGHT_WARN_THRESHOLD = 200

// 用 Map 维护登记顺序（Map 保持插入顺序，遍历时最早登记的在最前）
const inflightControllers = new Map<AbortController, true>()
let inflightCount = 0
let inflightDropped = 0 // 累计因触顶而跳过登记的次数
const inflightCountListeners = new Set<(n: number) => void>()

function notifyInflightChanged(): void {
  // Bug P0-B3 (fix): 同步遍历 Set 时 listener 内可能反向调 add/removeInflight，
  // 触发 notifyInflightChanged 重入——内层 for...of 遍历同一个 Set 的 iterator
  // 在并发修改下是 well-defined 但会无限递归。改为先 snapshot 成 Array 再遍历，
  // 重入只把新事件压到下一轮 notify（同一 tick 内多次 set 状态合并）。
  const fns = Array.from(inflightCountListeners)
  for (const fn of fns) {
    try { fn(inflightCount) } catch { /* listener 抛错不影响其他监听 */ }
  }
}

/** 注册一个在飞请求的 AbortController（请求拦截器调用） */
export function addInflight(controller: AbortController): void {
  // W-3 修复：触顶时"跳过登记"，绝不 abort 已有请求。
  // 旧实现是 `while (size >= MAX) { abort(最早的); delete }`——被杀掉的是正常业务
  // 请求，且静默失败（axios.isCancel 分支不提示不记日志）。宁可让本请求不受
  // abortAllInflight 管辖（它仍会正常发完并返回），也不能误杀别人的请求。
  if (inflightControllers.size >= MAX_INFLIGHT) {
    inflightDropped++
    // 只在首次与每 50 次打印，避免日志洪水
    if (inflightDropped === 1 || inflightDropped % 50 === 0) {
      logWarn(
        'inflight',
        `在飞请求数已达上限 ${MAX_INFLIGHT}，本次跳过登记（累计 ${inflightDropped} 次）；` +
        '若频繁触发请检查是否有未配对的 addInflight/removeInflight'
      )
    }
    return
  }
  if (inflightCount >= INFLIGHT_WARN_THRESHOLD && inflightCount % 50 === 0) {
    // 周期性 warn（每 50 个一次，避免日志洪水）
    logWarn(
      'inflight',
      `在飞请求数 ${inflightCount} 接近上限 ${MAX_INFLIGHT}，已跳过登记 ${inflightDropped} 次；` +
      '若频繁触发请检查是否有未配对的 addInflight/removeInflight'
    )
  }
  inflightControllers.set(controller, true)
  inflightCount++
  notifyInflightChanged()
}

/** 注销已结束请求的 AbortController（响应/错误拦截器调用） */
export function removeInflight(controller: AbortController | undefined): void {
  if (!controller) return
  if (inflightControllers.delete(controller)) {
    inflightCount = Math.max(0, inflightCount - 1)
    notifyInflightChanged()
  }
}

/** 累计因触顶而跳过登记的次数（诊断用，**仅供测试**，业务代码请勿引用） */
export function getInflightDroppedCount(): number {
  return inflightDropped
}

/** 在飞请求登记上限（**仅供测试**，业务代码请勿引用） */
export const INFLIGHT_LIMIT = MAX_INFLIGHT

/** 订阅正在飞行请求数量变化。返回取消订阅函数。 */
export function onInflightCountChange(fn: (n: number) => void): () => void {
  inflightCountListeners.add(fn)
  fn(inflightCount)
  return () => { inflightCountListeners.delete(fn) }
}

/** 获取当前正在飞行请求数量（同步）。 */
export function getInflightCount(): number {
  return inflightCount
}

/**
 * 取消所有正在飞行的请求（切网关 / 登出 / 断网 / 组件强制重置场景使用）。
 * 切网关时若不先 abort，已被 Tauri 端发出的 HTTP 请求可能写入错误的网关。
 */
export function abortAllInflight(reason = 'aborted by gateway switch / logout'): void {
  for (const c of inflightControllers.keys()) {
    try { c.abort(reason) } catch { /* controller 已释放 */ }
  }
  // Bug 2.7: abort 后清空 Map + 同步计数，避免控制器已 abort 但 Map 仍占位
  inflightControllers.clear()
  if (inflightCount !== 0) {
    inflightCount = 0
    notifyInflightChanged()
  }
}

/**
 * 切换网关后触发页面刷新。
 *
 * 先 abortAllInflight 取消所有在飞请求（避免打到旧网关），再延迟 reload
 * 让 abort 信号传播 + 通知展示。由 gatewayConfig.ts setActiveGateway 调用。
 *
 * ROI: abort 后立即显示全屏遮罩"正在切换网关..."，避免旧网关数据闪一下造成误解。
 * 200ms 延迟内 abort 信号传播 + 用户看到切换状态，reload 成功后遮罩随页面刷新自然消失。
 */
export function triggerGatewaySwitchRefresh(): void {
  abortAllInflight('gateway switch: abort inflight then reload')

  // ROI: 全屏遮罩 — 用原生 DOM 操作（本函数是 API 层，不依赖 Vue runtime）
  const overlay = document.createElement('div')
  overlay.style.cssText = 'position:fixed;inset:0;z-index:99999;background:rgba(15, 23, 42, 0.7);backdrop-filter:blur(6px);display:flex;align-items:center;justify-content:center;color:#fff;font-size:1rem;font-family:system-ui;-webkit-font-smoothing:antialiased;'
  overlay.innerHTML = '<div style="display:flex;align-items:center;gap:12px;padding:20px 28px;background:rgba(255, 255, 255, 0.08);border-radius:12px;backdrop-filter:blur(12px);"><svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" style="animation:gw-spin 0.9s linear infinite;"><path d="M21 12a9 9 0 11-6.22-8.56"/><path d="M21 3v6h-6"/></svg><span>正在切换网关，刷新数据…</span></div><style>@keyframes gw-spin{to{transform:rotate(360deg)}}</style>'
  document.body.appendChild(overlay)

  setTimeout(() => {
    try {
      window.location.reload()
    } catch {
      // 非浏览器环境（SSR/测试）或 reload 失败时手动移除遮罩
      overlay.remove()
    }
  }, 200)
}
