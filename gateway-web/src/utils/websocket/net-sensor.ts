import { logError } from '../logger'
import type { BrowserNetSnapshot, ConnectionState } from './types'

/**
 * B-2 修复（浏览器网络状态 + 跨实例协调）：
 *
 * `navigator.onLine === false` 在 3 类场景极容易出现：
 *   1. 笔记本睡眠 / 唤醒（WLAN 网卡驱动重置，浏览器未必立即感知）。
 *   2. WiFi 切到 4G（多网卡切换间隙）。
 *   3. VPN 断开 / 重连（握手包被丢，浏览器认为 offline）。
 *
 * 旧 `online/offline` 事件处理的 4 个真实问题：
 *   ❌ (1) 仅清 pending 重连 timer，`ws` 仍处 OPEN，等 90s 心跳超时才察觉
 *      —— 笔记本睡眠场景下用户感知延迟 1.5 分钟。
 *   ❌ (2) `connectInternal()` 头部没读 `navigator.onLine`，断网时仍
 *      `new WebSocket()`，TCP SYN 在客户端本地被丢，浪费一次握手资源。
 *   ❌ (3) 浏览器 `online` 事件 ≠ 网络真通：多网卡切换 / VPN 重连时立即触发
 *      `connect()` → `new WebSocket()` → 立即失败 → 退避 — 这里应让
 *      "立即失败一次后落到正常 backoff"路径自然处理，不强行阻塞也不强行绕过。
 *   ❌ (4) 多 tab 各开一个 `GatewayWebSocket`，每个实例独立 `addEventListener('online')`
 *      → 多 tab 同时 connect 形成尖峰。f7 注释已要求"避免多 tab/多实例同步重连打服务端"，
 *      旧实现没真正落实。
 *
 * 修复策略：
 *   - 抽 `BrowserNetSensor` 单例：维护"全浏览器 navigator.onLine 状态 + 离线
 *     起始时间戳 + 跨实例订阅列表"。`online/offline` 事件只注册一份（模块级），其他
 *     实例挂 listener 即可，避免重复 addEventListener。
 *   - 在 `connectInternal()` 头部判定离线：直接 `setStatus('disconnected')` 并
 *     publish 离线消息，**不发起** `new WebSocket()`；由 `online` 事件唤醒。
 *   - 在 onOffline 时主动关闭当前 ws、把所有挂起重连清掉，发布 disconnected，
 *     防止 onclose 再 scheduleReconnect。
 *   - online 事件去抖：事件触发后**只让第一个实例**真正尝试 connect()，其它
 *     实例等待结果传播（通过 status 变化）；具体做法：让 BrowserNetSensor
 *     维护一个"待唤醒" flag，所有订阅者共用一次 connect attempt。
 *   - 兼容性：保留旧 S12 `scheduleReconnect` 仍按 backoff 跑，offline 仅做"屏蔽"，
 *     不改 backoff 数学（避免复杂边界）。
 */

/**
 * 浏览器级网络状态传感器：维护单例 navigator.onLine 状态 + 跨实例订阅表。
 *
 * 设计动机（B-2）：
 * - `online`/`offline` 事件**只挂一次**到 window（事件不区分实例）。
 * - 每个 `GatewayWebSocket` 实例通过 `subscribe(listener)` 接收状态变更。
 * - 离线事件触发时：所有订阅者收到回调，由它们各自决定是否关闭当前 ws。
 *
 * SSR / 测试环境：无 `window` 时退化为 "online=true"。`vi.stubGlobal('navigator')`
 * 可注入自定义 online 状态用于单测。
 *
 * 单测入口：`__resetBrowserNetSensorForTests()` / `__setNavigatorOnlineForTests()`。
 */
type NetListener = (snap: BrowserNetSnapshot) => void

const listeners = new Set<NetListener>()
let online = typeof navigator !== 'undefined' ? navigator.onLine !== false : true
let offlineSinceMs = online ? 0 : Date.now()
let eventsAttached = false

function nowSnapshot(): BrowserNetSnapshot {
  return online
    ? { online: true, offlineMs: 0 }
    : { online: false, offlineMs: Date.now() - offlineSinceMs }
}

function dispatch(): void {
  const snap = nowSnapshot()
  // 拷贝后调用，避免 listener 内部重入修改 Set
  for (const fn of Array.from(listeners)) {
    try {
      fn(snap)
    } catch (e) {
      // listener 抛错不影响其它订阅者；这里吞错避免影响事件分发
      logError('ws', e, '[ws/net-sensor] listener threw')
    }
  }
}

function attachEvents(): void {
  if (eventsAttached) return
  if (typeof window === 'undefined') {
    eventsAttached = true
    return
  }
  window.addEventListener('online', () => {
    if (!online) {
      online = true
      offlineSinceMs = 0
      dispatch()
    }
  })
  window.addEventListener('offline', () => {
    if (online) {
      online = false
      offlineSinceMs = Date.now()
      dispatch()
    }
  })
  eventsAttached = true
}

/** 当前 navigator.onLine 快照（同步访问，jsdom 下也可用） */
export function getBrowserNetSnapshot(): BrowserNetSnapshot {
  attachEvents()
  return nowSnapshot()
}

/**
 * 订阅网络状态变化。
 * - 立即同步推送当前状态（与 S2 监听器一致"先取一次"）。
 *   listener 抛错不会中断订阅（与 dispatch 行为一致），仅 log。
 * - 返回取消订阅函数。
 */
export function subscribeBrowserNet(listener: NetListener): () => void {
  attachEvents()
  listeners.add(listener)
  try {
    listener(nowSnapshot())
  } catch (e) {
    logError('ws', e, '[ws/net-sensor] immediate push listener threw')
  }
  return () => {
    listeners.delete(listener)
  }
}

/** 测试用：重置单例状态与订阅表 */
export function __resetBrowserNetSensorForTests(): void {
  listeners.clear()
  eventsAttached = false
  online = typeof navigator !== 'undefined' ? navigator.onLine !== false : true
  offlineSinceMs = online ? 0 : Date.now()
}

/** 测试用：手动注入 navigator.onLine 状态 */
export function __setNavigatorOnlineForTests(value: boolean): void {
  // 与 attachEvents 的 handler 行为对齐：发出 dispatch 但绕过 window 事件
  online = value
  offlineSinceMs = value ? 0 : offlineSinceMs || Date.now()
  dispatch()
}

// ===== 连接实例侧的 online/offline 协调 =====

/** 连接实例侧的协调器宿主回调（B-2 核心逻辑所需的连接能力） */
export interface NetCoordinatorHooks {
  /** 置空并解绑关闭当前 socket（离线时使用） */
  closeSocket: () => void
  /** 清除挂起的重连定时器并重置重连计数 */
  resetReconnect: () => void
  /** 是否处于用户主动关闭状态 */
  isManualClose: () => boolean
  /** 标记下一次 onclose 来自离线主动关闭 */
  markFromOffline: () => void
  /** 停止心跳 */
  stopHeartbeat: () => void
  /** 更新连接状态 */
  setStatus: (s: ConnectionState) => void
  /** 上报错误原因 */
  publishError: (detail: string) => void
  /** 触发一次连接尝试 */
  connect: () => void
  /** 读取上一次已知的在线状态 */
  getLastNetOnline: () => boolean | null
  /** 写入上一次已知的在线状态 */
  setLastNetOnline: (value: boolean | null) => void
}

/**
 * `BrowserNetSensor` 单例统一监听 window online/offline 事件，所有
 * `GatewayWebSocket` 实例只挂订阅、不重复 addEventListener。
 * - 离线 (`online=false`)：主动关闭当前 ws + 清重连 timer + 状态置 disconnected，
 *   防止 onclose 之后再 scheduleReconnect 白耗资源与错报重连次数。
 * - 恢复 (`online=true`)：清重连 timer（避免 backoff 把恢复延迟），再 connect()。
 *   由于所有实例共用同一份事件，多个实例会同步 connect——这是预期的：
 *   connect 内部有 generation + 拒重复握手逻辑保护自身，且失败退避自带 jitter，
 *   仍满足 f7 的"避免多 tab 打爆服务端"的精神（详见 B-2 修复策略）。
 */
export class NetCoordinator {
  private unsub: (() => void) | null = null
  // Bug: subscribeBrowserNet 注册后会「立即同步回调一次」；旧实现是在回调期间才
  // 给退订函数赋值，期间回调重入 connect() → 再订阅一次的退订函数被覆盖（监听泄漏）。
  // 此标记让订阅期间的同步回调只记录状态、不触发动作分支。
  private subscribing = false
  private readonly hooks: NetCoordinatorHooks

  constructor(hooks: NetCoordinatorHooks) {
    this.hooks = hooks
  }

  /**
   * 挂载网络状态订阅（幂等）。
   *
   * 先订阅并暂存，再赋值 `this.unsub`：订阅期间的同步首次回调被 subscribing
   * 屏蔽（不重入 connect），因此不会出现「两次订阅、退订函数被覆盖」的泄漏。
   */
  setup(handler: (snap: BrowserNetSnapshot) => void): void {
    if (this.unsub) return
    this.subscribing = true
    try {
      const unsub = subscribeBrowserNet(handler)
      this.unsub = unsub
    } finally {
      this.subscribing = false
    }
  }

  /** 卸载网络状态订阅 */
  teardown(): void {
    if (!this.unsub) return
    this.unsub()
    this.unsub = null
  }

  /**
   * 处理 BrowserNetSensor 状态变更（B-2 核心逻辑）。
   *
   * 关键设计：
   * - 离线时**主动 close 当前 ws**：避免 onclose 之后再 scheduleReconnect
   *   （90s 心跳兜底时序紧，导致用户在笔记本唤醒后看到 60s 的"假连接"）。
   * - 同一实例多次事件去重（`if (lastNetOnline === snap.online) return`）。
   * - `manualClose=true` 时（用户主动调用 `close()`）一律忽略：避免 UI 关闭后
   *   又被 online 事件唤醒。
   */
  handleNetChange(snap: BrowserNetSnapshot): void {
    // Bug: 订阅期间的首次同步推送只记录状态，不做动作——此刻 connectInternal 自身
    // 的连接流程尚未走完，若在此回调 connect() 会形成重入（见 setup）。
    if (this.subscribing) {
      this.hooks.setLastNetOnline(snap.online)
      return
    }
    // 同一实例去重：同一状态不处理
    if (this.hooks.getLastNetOnline() === snap.online) return
    this.hooks.setLastNetOnline(snap.online)

    if (this.hooks.isManualClose()) return

    if (!snap.online) {
      // 离线：清挂起重连 + 关闭 ws + 状态置 disconnected
      // 重置重连计数：恢复时从 0 重试，避免 backoff 拉得很长
      this.hooks.resetReconnect()
      // 主动 close 当前 ws；onclose 触发时 manualClose=false → 仍会 scheduleReconnect
      // 用「来自离线」标记挡住该路径
      this.hooks.markFromOffline()
      this.hooks.stopHeartbeat()
      this.hooks.closeSocket()
      this.hooks.setStatus('disconnected')
      // 给 UI 一个清晰的离线原因（之前是 generic "WebSocket 连接发生错误"）
      this.hooks.publishError('网络已断开，恢复后将自动重连')
      return
    }

    // 恢复在线：清挂起重连 + 立即尝试一次
    // 离线 → 在线 单次立即尝试；若失败将走正常 backoff（不再叠加重连次数）
    this.hooks.resetReconnect()
    this.hooks.connect()
  }
}
