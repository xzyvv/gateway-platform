/**
 * 401 登出时的 auth store 补丁桥
 *
 * ## 为什么不用直接 import
 *
 * `api/http/responseInterceptor.ts` 静态引 `stores/auth.ts`，而 `stores/auth.ts`
 * 又 `from '@/api'` 回指 api barrel —— 闭合成**静态循环依赖**（5 组环的共同根因，
 * 见 `docs/audit-2026-11/17-前端第五轮技术债务摸底.md` A-1 / DD-65）。
 *
 * ## 为什么不用动态 import
 *
 * `stores/auth.ts` 已被 `router/index.ts`、`components/HubShell.vue` 等多处静态引入，
 * 与 `api/http/` 同在入口 chunk。动态导入会产生 `INEFFECTIVE_DYNAMIC_IMPORT`
 * 构建警告（零拆包收益）——同一顾虑见 `api/inflight.ts` 头部注释。
 *
 * ## 做法
 *
 * 由 `app/main.ts` 在 pinia 安装后注册一个补丁函数，传输层只管调用。
 * 保持**同步**语义：登出时 auth store 的 `loggedIn` 必须立刻变 false，
 * 否则 UI 会短暂停留在「已登录」态。
 */

/** 登出时对 auth store 施加的补丁 */
export interface AuthLogoutPatch {
  loggedIn: boolean
  token: string
}

type AuthPatcher = (patch: AuthLogoutPatch) => void

let patcher: AuthPatcher | null = null

/**
 * 注册 auth store 的登出补丁。
 *
 * 调用方：`app/main.ts`（pinia 就绪后）。传 `null` 可注销补丁（单测复位用）。
 */
export function setAuthLogoutPatcher(fn: AuthPatcher | null): void {
  patcher = fn
}

/**
 * 施加登出补丁。
 *
 * 未注册（pinia 未激活 / 传输层单测）时静默跳过——与原实现的
 * `try { useAuthStore().$patch(...) } catch { /* pinia 未激活 *\/ }` 行为一致。
 */
export function applyAuthLogoutPatch(): void {
  if (!patcher) return
  try {
    patcher({ loggedIn: false, token: '' })
  } catch {
    /* pinia 未激活 */
  }
}
