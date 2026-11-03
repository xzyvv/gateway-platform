import { onMounted, onUnmounted, onActivated, onDeactivated, ref, type Ref } from 'vue'

export interface UsePollingOptions {
  /**
   * 默认 true：onMounted 自动 start 并立即执行一次 fn，
   * 等价旧写法 onMounted(() => { load(); timer = setInterval(load, n) })。
   * 开关式自动刷新（用户开 toggle 才启动）传 false，自行调 start/stop。
   */
  autoStart?: boolean
  /** 默认 true：标签页隐藏时暂停轮询，重新可见时立即补执行一次 */
  pauseWhenHidden?: boolean
  /**
   * 默认 true：fn 抛错时按 interval × 2^连续失败次数 退避，封顶 maxInterval，
   * 成功后恢复 interval。注意：fn 内部自行 try/catch 吞掉错误的调用方
   * 感知不到失败，不会触发退避（可见性暂停仍生效）。
   */
  backoff?: boolean
  /** 退避上限，默认 min(interval × 8, 60s) */
  maxInterval?: number
  /**
   * 默认 true：组件被 KeepAlive 缓存而「切走」（deactivated）时暂停轮询，
   * 切回（activated）时恢复。
   *
   * 背景：KeepAlive 只把组件移入缓存容器，**不会触发 onUnmounted**，
   * 于是 onUnmounted 里的定时器清理不执行——切到别的 tab 后，被缓存页面的
   * 轮询仍在后台跑（报文捕获 500ms 一次）。onDeactivated 在切走时停表、
   * onActivated 在切回时补一次，正好对齐 KeepAlive 的生命周期。
   *
   * 非 KeepAlive 场景下这两个钩子不会触发，行为与之前完全一致。
   */
  pauseWhenDeactivated?: boolean
  /**
   * fn 抛错（同步抛出或 Promise reject）时的旁路回调，默认不处理。
   *
   * 调度器本身只把失败用于退避计数，不负责提示；展示型订阅者（useSharedTick）
   * 需要把异常记进统一日志，就在此处注入。
   */
  onError?: (e: unknown) => void
}

export interface UsePollingReturn {
  /** 开始轮询（幂等，重复调用不叠加定时器）；runNow=true 时先立即执行一次 */
  start: (runNow?: boolean) => void
  stop: () => void
  /** 当前是否在轮询 */
  running: Readonly<Ref<boolean>>
  /** 调整本订阅者的轮询周期（毫秒，下限 100）；用于按数据新鲜度降频 */
  setInterval: (ms: number) => void
}

/** 一个轮询订阅者：由当前生效周期 + 上次执行时刻（虚拟时钟）共同决定本轮是否轮到它 */
interface PollSubscriber {
  fn: () => void | Promise<unknown>
  /** 基础周期（成功态） */
  interval: number
  /** 当前生效周期（含失败退避；成功后回到 interval） */
  activeDelay: number
  /** 退避上限 */
  maxInterval: number
  /** 是否启用失败退避 */
  backoff: boolean
  /** 标签页隐藏时是否暂停 */
  pauseWhenHidden: boolean
  /**
   * KeepAlive 切走（deactivated）时是否暂停。
   * 与 pauseWhenHidden 是两个独立维度：标签页可见但组件已切走，同样该停。
   */
  pauseWhenDeactivated: boolean
  /** 当前是否处于 KeepAlive 缓存状态（被切走） */
  deactivated: boolean
  /** 连续失败次数（驱动退避，成功清零） */
  failures: number
  /** fn 执行中守卫：fn 耗时超过周期时跳过重入 */
  ticking: boolean
  /** 上次执行的虚拟时刻（vNow 单调累加，见下） */
  lastVRun: number
  /** 对外暴露的 running 标志 */
  running: Ref<boolean>
  /** 失败旁路回调（见 UsePollingOptions.onError） */
  onError?: (e: unknown) => void
}

/* ========== 共享调度器（模块级单例） ==========
 * 全站所有 usePolling 实例共享**一个** setInterval：同页多个轮询（如 BusLoad 多卡片
 * + 概要刷新）不再各挂一个定时器各管各的唤醒。周期取所有订阅者当前生效周期
 * （含退避）的最小值，每个订阅者按自己的到期时刻触发——某个订阅者退避降频
 * 不影响别人，全员退避时全局周期也随之放宽。
 *
 * 全局闸门（两道，防"多页同时开着"把网关压垮）：
 * 1. GLOBAL_MIN_INTERVAL_MS：全局定时器周期不低于该值，任何订阅者都无法以更快的
 *    频率被唤醒（写死的 100ms 轮询会被抬到全局下限）；
 * 2. MAX_CONCURRENT_POLLS：同一时刻在飞的回调数上限，到顶后本轮直接跳过其余
 *    订阅者，留到下一个 tick 再试（不丢轮次，只延后一拍）。
 *
 * 到期判断用「虚拟时钟」而非 Date.now()：既有页面单测只伪造 setInterval/clearInterval
 * （flushPromises 依赖真实 setTimeout），Date.now() 在 fake timers 下不随
 * advanceTimersByTime 推进。做法：全局 tick 一次 vNow += timerPeriod，各订阅者
 * 比较 vNow - lastVRun >= activeDelay——时间语义与旧版「每实例独立
 * setInterval(activeDelay)」逐点对齐（含退避后的周期重排），既有单测无需改动。
 *
 * 调度器仍用 setInterval 而非 setTimeout 链——保持与既有单测的 fake 范围兼容。
 */

/** 全局最小轮询间隔（毫秒）：任何订阅者的实际触发频率都不会快于它 */
export const GLOBAL_MIN_INTERVAL_MS = 250

/**
 * 全局并发上限：同一时刻最多允许多少个轮询回调在飞。
 *
 * 取值偏保守：网关 Web 层单机并发能力有限，现场常见的「一个屏幕开四五个卡片」
 * 场景下 4 路并发已能保证首屏体感，多出来的订阅者顺延到下一拍而不是一起打出去。
 */
export const MAX_CONCURRENT_POLLS = 4

const subscribers = new Set<PollSubscriber>()
let timer: ReturnType<typeof setInterval> | null = null
/** 当前全局定时器周期；与期望周期一致时不重建，避免每次订阅变更都打乱相位 */
let timerPeriod = 0
/** 虚拟时钟：每收到一次全局 tick 前进 timerPeriod 毫秒 */
let vNow = 0
let watchingVisibility = false

function isHidden(): boolean {
  return typeof document !== 'undefined' && document.visibilityState === 'hidden'
}

/** 当前在飞的轮询回调数（并发闸门用） */
let inFlight = 0
/** 每拍轮转起点，避免并发闸门长期饿死排在后面的订阅者 */
let rotation = 0

function clearTimer(): void {
  if (timer !== null) {
    clearInterval(timer)
    timer = null
  }
  timerPeriod = 0
}

/** 重建全局定时器：周期取所有订阅者当前生效周期（含退避）的最小值 */
function syncTimer(): void {
  if (subscribers.size === 0) {
    clearTimer()
    return
  }
  if (isHidden()) {
    // 隐藏时仅当存在 pauseWhenHidden=false 的订阅者才保留定时器（由 fire 逐个跳过暂停者）；
    // 当前项目所有调用方都未关闭该选项，此处保留开关语义
    let allPause = true
    for (const s of subscribers) {
      if (!s.pauseWhenHidden) { allPause = false; break }
    }
    if (allPause) {
      clearTimer()
      return
    }
  }
  let min = Number.POSITIVE_INFINITY
  for (const s of subscribers) {
    // 被 KeepAlive 切走的订阅者不参与周期计算：否则一个 500ms 的缓存页
    // 会把全局周期钉在 500ms，即使当前可见页面都只要 5s 一次
    if (s.pauseWhenDeactivated && s.deactivated) continue
    if (s.activeDelay < min) min = s.activeDelay
  }
  // 所有订阅者都被切走（或都不存在）→ 无需定时器。
  // 注意不能退化成 period=0：setInterval(0) 是 0ms 空转，会把主线程跑满。
  if (!Number.isFinite(min)) {
    clearTimer()
    return
  }
  const period = Math.max(min, GLOBAL_MIN_INTERVAL_MS)
  if (timer !== null && period === timerPeriod) return
  clearTimer()
  timerPeriod = period
  timer = setInterval(onGlobalFire, period)
}

/** 判断 fn 返回值是否可等待：同步回调当场收尾，异步回调等 settle 后再收尾 */
function isThenable(value: unknown): value is PromiseLike<unknown> {
  if (typeof value !== 'object' && typeof value !== 'function') return false
  return typeof (value as { then?: unknown }).then === 'function'
}

/**
 * 收尾一个订阅者：释放并发额度、清零/累计失败、按需驱动退避。
 *
 * 同步回调必须当场收尾（不能等微任务）：共享调度器一次 onGlobalFire 可能
 * 连发多拍，若 ticking/inFlight 要等微任务才复位，后续拍会被『执行中』守卫跳过。
 */
function settle(s: PollSubscriber, failed: boolean): void {
  s.ticking = false
  inFlight--
  s.failures = failed ? Math.min(s.failures + 1, 10) : 0
  if (!s.backoff) return
  const want = s.failures === 0 ? s.interval : Math.min(s.interval * 2 ** s.failures, s.maxInterval)
  if (want !== s.activeDelay) {
    s.activeDelay = want
    // 退避可能改变全局最小周期（如唯一的订阅者降频），重建定时器
    syncTimer()
  }
}

/** 执行一个订阅者：记录虚拟时刻，同步/异步回调分别收尾 */
function runSubscriber(s: PollSubscriber): void {
  s.lastVRun = vNow
  s.ticking = true
  inFlight++
  let result: void | Promise<unknown>
  try {
    result = s.fn()
  } catch (e) {
    s.onError?.(e)
    settle(s, true)
    return
  }
  if (isThenable(result)) {
    result.then(
      () => settle(s, false),
      (e: unknown) => {
        s.onError?.(e)
        settle(s, true)
      },
    )
    return
  }
  settle(s, false)
}

function onGlobalFire(): void {
  vNow += timerPeriod
  // 快照遍历：回调里可能 start/stop/退避改周期，不能边遍历边改集合
  const list = Array.from(subscribers)
  // 轮转起点：并发闸门挡下的是「排在后面的」订阅者，固定从 0 开始会让末尾
  // 订阅者长期饿死；每拍把起点前移一格，保证多订阅者下人人有份
  const len = list.length
  const start = len > 0 ? rotation++ % len : 0
  for (let i = 0; i < len; i++) {
    const s = list[(start + i) % len]
    if (!s || !subscribers.has(s)) continue
    if (s.pauseWhenHidden && isHidden()) continue
    // KeepAlive 切走：组件仍在缓存里，但用户看不到，不该继续发请求
    if (s.pauseWhenDeactivated && s.deactivated) continue
    if (s.ticking) continue
    if (vNow - s.lastVRun < s.activeDelay) continue
    // 并发闸门：在飞回调已达上限时本轮不再发起，留到下一拍（lastVRun 未更新，下拍仍到期）
    if (inFlight >= MAX_CONCURRENT_POLLS) continue
    runSubscriber(s)
  }
}

function onVisibilityChange(): void {
  if (typeof document === 'undefined') return
  if (document.visibilityState === 'hidden') {
    // 全员暂停 → 停表；存在不暂停者 → 保留，由 fire 逐个跳过
    syncTimer()
    return
  }
  if (subscribers.size === 0) return
  // 回到前台：重置退避并立即补一次，避免用户盯着过期数据（与旧实现一致）
  for (const s of Array.from(subscribers)) {
    s.failures = 0
    s.activeDelay = s.interval
  }
  syncTimer()
  for (const s of Array.from(subscribers)) {
    if (s.ticking) continue
    // 回前台补跑同样受并发闸门约束，避免一次唤醒发出十几个请求
    if (inFlight >= MAX_CONCURRENT_POLLS) break
    runSubscriber(s)
  }
}

function watchVisibility(): void {
  if (watchingVisibility || typeof document === 'undefined') return
  document.addEventListener('visibilitychange', onVisibilityChange)
  watchingVisibility = true
}

function unwatchVisibility(): void {
  if (!watchingVisibility || typeof document === 'undefined') return
  document.removeEventListener('visibilitychange', onVisibilityChange)
  watchingVisibility = false
}

/**
 * 统一轮询 composable，替代各页面裸 setInterval。解决四个全站问题：
 * 1. 标签页切后台后仍按原频率轮询——网关是低功耗工控设备，多个调试页
 *    同时挂着时白耗资源；
 * 2. 网关失联后按原频率打注定失败的请求——失败时指数退避降频；
 * 3. 同页多个轮询各自挂定时器——全站共享一个调度器（见上方模块注释）；
 * 4. KeepAlive 缓存页切走后仍在后台轮询——切走停表、切回补一次。
 */
export function usePolling(
  fn: () => void | Promise<unknown>,
  interval: number,
  options: UsePollingOptions = {},
): UsePollingReturn {
  const {
    autoStart = true,
    pauseWhenHidden = true,
    backoff = true,
    maxInterval = Math.min(interval * 8, 60_000),
    pauseWhenDeactivated = true,
  } = options

  const running = ref(false)
  const subscriber: PollSubscriber = {
    fn,
    interval,
    activeDelay: interval,
    maxInterval,
    backoff,
    pauseWhenHidden,
    pauseWhenDeactivated,
    deactivated: false,
    failures: 0,
    ticking: false,
    lastVRun: 0,
    running,
    onError: options.onError,
  }

  function start(runNow = false): void {
    // 定时器单例：等价旧代码的 if (!timer) 守卫（Bug 178 教训）
    if (subscribers.has(subscriber)) return
    // 计时起点取注册时刻：首轮在满一个周期后才触发（调用方通常自己先做一次同步加载）
    subscriber.lastVRun = vNow
    subscribers.add(subscriber)
    running.value = true
    watchVisibility()
    syncTimer()
    // 挂载即跑也要过全局并发闸门：一次路由切换可能同时挂载多个卡片，
    // 超额部分不丢，交给下一拍由全局 tick 正常补跑
    if (runNow && inFlight < MAX_CONCURRENT_POLLS) runSubscriber(subscriber)
  }

  function stop(): void {
    subscribers.delete(subscriber)
    running.value = false
    syncTimer()
    if (subscribers.size === 0) unwatchVisibility()
  }

  /**
   * 调整本订阅者周期（毫秒，下限 100）。
   * 重置计时起点：刚降频时不该沿用旧周期的进度，否则会提前或推迟一次。
   */
  function setInterval(ms: number): void {
    const next = Math.max(100, Math.round(ms))
    if (next === subscriber.interval) return
    subscriber.interval = next
    subscriber.activeDelay = next
    subscriber.failures = 0
    subscriber.lastVRun = vNow
    // 周期变大后原退避上限可能小于基础周期，保证「上限 ≥ 基础周期」
    if (subscriber.maxInterval < next) subscriber.maxInterval = next
    syncTimer()
  }

  onMounted(() => {
    if (autoStart) start(true)
  })

  onUnmounted(() => {
    stop()
  })

  // KeepAlive 生命周期：切走（deactivated）停表，切回（activated）补一次。
  // 非 KeepAlive 场景下这两个钩子不会触发，行为与之前完全一致。
  if (pauseWhenDeactivated) {
    // Vue 在 KeepAlive 内**首次挂载**时也会触发 activated（见 runtime-core 的
    // instance.a 排队逻辑），此时并不存在「切走再切回」，不能补跑一次，
    // 否则挂载就会连发两次请求（onMounted 已 start(true) 跑过一次）。
    // 用这个标记区分「首次激活」与「从缓存恢复」。
    let restoredFromCache = false
    onDeactivated(() => {
      subscriber.deactivated = true
      restoredFromCache = true
      // 周期可能因「移除该订阅者」而放宽（它常常是全局最快的一个）
      syncTimer()
    })
    onActivated(() => {
      subscriber.deactivated = false
      // 首次激活：onMounted 的 start(true) 已经跑过一次，不重复
      if (!restoredFromCache) return
      restoredFromCache = false
      // 尚未 start（autoStart:false 的开关式轮询）时不该擅自发起请求
      if (!subscribers.has(subscriber)) return
      // 切回时重置退避：缓存期间的状态可能早已过时
      subscriber.failures = 0
      subscriber.activeDelay = subscriber.interval
      syncTimer()
      // 立刻补一次：用户切回来就看到最新的数据，而不是等下一个周期
      if (!subscriber.ticking) runSubscriber(subscriber)
    })
  }

  return { start, stop, running: running as Readonly<Ref<boolean>>, setInterval }
}

/**
 * 测试用：清空订阅并停表，等价于「所有组件都已卸载」。
 * 共享调度器是模块级状态：残留的定时器句柄/订阅会让跨用例状态泄漏
 * （与 useSharedTick 的重置同一策略，见 src/test/setup.ts）。
 */
export function __resetUsePollingForTests(): void {
  subscribers.clear()
  clearTimer()
  unwatchVisibility()
  vNow = 0
  // 在飞计数/轮转起点也是模块级状态：用例里若有未完成的回调被卸载丢弃，
  // 残留额度会挤占后续用例的并发闸门
  inFlight = 0
  rotation = 0
}
