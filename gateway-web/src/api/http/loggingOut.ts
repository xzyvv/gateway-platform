/**
 * 401 单飞登出闸门（logging-out 引用计数）
 *
 * Bug 2.10 (2026-Q1) 修复：401 单飞登出由 boolean 改 ref-counted
 *
 * 旧实现 `let isLoggingOut = false` 是模块级布尔，理论上是 set 一次就互斥，
 * 但**多 401 并发**场景下（网关重启后所有在飞请求同时 401）：
 *   1. 请求 A 的拦截器看到 isLoggingOut=false → set true + clearToken + 弹通知 + 跳登录
 *   2. 请求 B（几乎同时）也看到 isLoggingOut=false → set true + 再次 clearToken + 再次弹通知 + 再次跳登录
 * 旧 boolean 看似互斥，但**set 与读不是原子**——微秒级的并发仍能让多个 401 看到 false。
 *
 * 修复：用 ref-counted (acquire/release)，并配套 idle 状态避免清理期间再有
 * 401 触发弹窗。仅**第一个 401** 走"弹通知+跳登录"，后续走"静默丢弃"。
 *
 * 状态机：
 * - idle       : 0，未在登出流程，可接受新 401 触发流程
 * - active (n>0): 至少一个 401 正在触发登出，新 401 静默丢弃
 * release 后回到 idle。
 */
let loggingOutCount = 0

/**
 * 主动登出抑制位（W-2）。
 *
 * `auth.logout()` 是用户主动登出，此时 token 已清，但**在途请求仍会拿到 401**。
 * 若不抑制，每个 401 都会走"弹【鉴权失败，请重新登录】+ 跳登录页"路径，
 * 用户明明是自己点的退出，却看到一堆鉴权失败提示。
 *
 * 与 ref-count 分离的原因：ref-count 要求 acquire/release 严格配对，而
 * `markLoggingOut()` 是"标记"语义、没有配对的 release。旧实现借用
 * `tryAcquireLoggingOut()` 来完成标记，在"已有 401 正在登出"（count>0）分支
 * 只自增、不释放 → count 永远回不到 0 → 此后所有 401 都被判为"已有人在登出"
 * 而静默丢弃，**再也不会跳登录页**，页面卡在"看着已登录、请求全 401"的状态
 * （只有下次登录成功调用 `resetLoggingOut()` 才复位）。
 *
 * 因此改用独立布尔位：置位不碰 ref-count，由登录成功时的 `resetLoggingOut()`
 * 一并清除，生命周期与"本次会话"一致。
 */
let loggingOutSuppressed = false

/**
 * 尝试获取"登出门"（ref-counted）。
 * @returns true 表示当前**第一个**触发登出（应走弹窗+跳登录路径），
 *          false 表示已有人在登出，调用方应走"静默丢弃"路径。
 */
export function tryAcquireLoggingOut(): boolean {
  // 主动登出期间：一律静默。仍然自增计数，保证与调用方的 release 严格配对。
  if (loggingOutSuppressed) {
    loggingOutCount += 1
    return false
  }
  if (loggingOutCount === 0) {
    loggingOutCount = 1
    return true
  }
  loggingOutCount += 1
  return false
}

/** 释放"登出门"（ref-counted decrement；最后一个 release 后才重置） */
export function releaseLoggingOut(): void {
  if (loggingOutCount > 0) {
    loggingOutCount -= 1
  }
}

/**
 * 兼容旧 API：标记"进入登出流程"（主动登出）。
 *
 * 只置抑制位、**不占用引用计数**——占用请用 tryAcquireLoggingOut/releaseLoggingOut
 * 配对。抑制一直持续到下次登录成功时的 resetLoggingOut()。
 */
export function markLoggingOut(): void {
  loggingOutSuppressed = true
}

/** 兼容旧 API：resetLoggingOut() 强清零（用于 login 成功后兜底） */
export function resetLoggingOut(): void {
  loggingOutCount = 0
  loggingOutSuppressed = false
}

/** 仅诊断/测试用：当前登出门引用计数（不含抑制位）。业务代码请勿引用。 */
export function getLoggingOutCount(): number {
  return loggingOutCount
}
