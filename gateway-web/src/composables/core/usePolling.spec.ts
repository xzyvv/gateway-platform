import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h, ref, nextTick, KeepAlive } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { usePolling, GLOBAL_MIN_INTERVAL_MS, type UsePollingOptions, type UsePollingReturn } from './usePolling'

/** 在组件 setup 中挂载 usePolling，返回包装器与 composable 句柄 */
function mountPolling(fn: () => void | Promise<unknown>, interval = 1000, options: UsePollingOptions = {}) {
  let api: UsePollingReturn | undefined
  const Comp = defineComponent({
    setup() {
      api = usePolling(fn, interval, options)
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  return { wrapper, api: api! }
}

function setVisibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true })
  document.dispatchEvent(new Event('visibilitychange'))
}

describe('usePolling', () => {
  beforeEach(() => {
    // 只伪造 setInterval，保留真实 setTimeout（flushPromises 依赖它）
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    setVisibility('visible')
  })

  afterEach(() => {
    vi.useRealTimers()
    setVisibility('visible')
  })

  it('autoStart 默认开启：挂载立即执行一次，之后按 interval 轮询', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { wrapper } = mountPolling(fn, 1000)
    expect(fn).toHaveBeenCalledTimes(1)
    await flushPromises() // 等首次 tick 完成，否则 ticking 守卫会跳过后续触发

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2)

    // 单次推进 2000 会同步连发两次 interval、第二次被 ticking 守卫跳过；
    // 分两次推进并在中间 flush，贴近真实时序（真实环境 fn 早已 resolve）
    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(3)
    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(4)
    wrapper.unmount()
  })

  it('autoStart=false 时不启动，start() 后才轮询，且重复 start 不叠加定时器', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { wrapper, api } = mountPolling(fn, 1000, { autoStart: false })
    expect(fn).not.toHaveBeenCalled()

    api.start()
    api.start() // 幂等：Bug 178 教训，定时器必须单例
    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1)

    api.stop()
    vi.advanceTimersByTime(3000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('标签页隐藏时暂停，重新可见时立即补一次并恢复轮询', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { wrapper } = mountPolling(fn, 1000)
    expect(fn).toHaveBeenCalledTimes(1)

    setVisibility('hidden')
    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 隐藏期间不轮询

    setVisibility('visible')
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2) // 回前台立即补一次

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(3) // 恢复周期轮询
    wrapper.unmount()
  })

  it('fn 连续抛错时指数退避，成功后恢复原间隔', async () => {
    const fn = vi.fn().mockRejectedValue(new Error('down'))
    const { wrapper } = mountPolling(fn, 1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 首次失败后立即退避到 2s

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 退避中，1s 点不触发

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2) // 2s 点触发 → 退避到 4s

    vi.advanceTimersByTime(2000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2) // 4s 未到

    vi.advanceTimersByTime(2000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(3) // 4s 点触发 → 退避到 8s

    // 恢复成功：下一次 tick 后回到 1s 间隔
    fn.mockResolvedValue(undefined)
    vi.advanceTimersByTime(8000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(4)

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(5) // 已恢复 1s 间隔
    wrapper.unmount()
  })

  it('回前台时重置退避状态', async () => {
    const fn = vi.fn().mockRejectedValue(new Error('down'))
    const { wrapper } = mountPolling(fn, 1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 已退避到 2s

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 确认处于退避中

    fn.mockResolvedValue(undefined)
    setVisibility('hidden')
    setVisibility('visible')
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2) // 回前台立即补一次

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(3) // 退避已重置，1s 间隔
    wrapper.unmount()
  })

  it('组件卸载后定时器停止，不再触发', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { wrapper } = mountPolling(fn, 1000)
    wrapper.unmount()

    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 仅挂载时的一次
  })

  it('fn 执行超过 interval 时不重入', async () => {
    let resolveHang: (() => void) | null = null
    const fn = vi.fn().mockImplementation(
      () => new Promise<void>((resolve) => { resolveHang = resolve }),
    )
    const { wrapper } = mountPolling(fn, 1000)

    // 首次调用挂起；推进多个周期不应叠加新调用
    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1)

    resolveHang!()
    await flushPromises()
    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2)
    // 收尾：第二次调用仍挂在 pending 上，必须放行——全局并发计数是模块级状态，
    // 泄漏一个未完成的回调会挤占后续用例的并发额度
    resolveHang!()
    await flushPromises()
    wrapper.unmount()
  })

  it('共享调度：两个不同周期的轮询只挂一个底层定时器，且各自按节奏触发', async () => {
    // 计数 setInterval 创建次数：两个订阅者的最小周期相同（1000），
    // 第二个订阅者加入时不得重建定时器
    const setIntervalSpy = vi.spyOn(globalThis, 'setInterval')
    const fnA = vi.fn().mockResolvedValue(undefined)
    const fnB = vi.fn().mockResolvedValue(undefined)
    let apiA: UsePollingReturn | undefined
    const Comp = defineComponent({
      setup() {
        apiA = usePolling(fnA, 1000)
        usePolling(fnB, 3000)
        return () => h('div')
      },
    })
    const wrapper = mount(Comp)
    // 挂载即各执行一次（autoStart）
    expect(fnA).toHaveBeenCalledTimes(1)
    expect(fnB).toHaveBeenCalledTimes(1)
    await flushPromises()

    // 两个订阅者共享一个 1000ms 定时器：只创建一次
    expect(setIntervalSpy).toHaveBeenCalledTimes(1)
    expect(setIntervalSpy.mock.calls[0]?.[1]).toBe(1000)

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fnA).toHaveBeenCalledTimes(2) // A 按 1s 触发
    expect(fnB).toHaveBeenCalledTimes(1) // B 的 3s 未到

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fnA).toHaveBeenCalledTimes(3)
    expect(fnB).toHaveBeenCalledTimes(1)

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fnA).toHaveBeenCalledTimes(4) // t=3000
    expect(fnB).toHaveBeenCalledTimes(2) // B 的 3s 到点

    // 停掉 A 不影响 B：A 的 1s 周期消失后全局周期放宽到 B 的 3s
    apiA!.stop()
    vi.advanceTimersByTime(3000)
    await flushPromises()
    expect(fnA).toHaveBeenCalledTimes(4)
    expect(fnB).toHaveBeenCalledTimes(3)
    wrapper.unmount()
    setIntervalSpy.mockRestore()
  })

  it('共享调度：一个订阅者退避降频不影响其他订阅者的周期', async () => {
    const failFn = vi.fn().mockRejectedValue(new Error('down'))
    const okFn = vi.fn().mockResolvedValue(undefined)
    const Comp = defineComponent({
      setup() {
        usePolling(failFn, 1000)
        usePolling(okFn, 1000)
        return () => h('div')
      },
    })
    const wrapper = mount(Comp)
    expect(failFn).toHaveBeenCalledTimes(1)
    expect(okFn).toHaveBeenCalledTimes(1)
    await flushPromises() // failFn 首败 → 退避到 2s

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(failFn).toHaveBeenCalledTimes(1) // 退避中，1s 点不触发
    expect(okFn).toHaveBeenCalledTimes(2) // 正常订阅者不受影响，仍按 1s 触发
    wrapper.unmount()
  })
})

/**
 * KeepAlive 场景：Hub 的 tab 切换用 KeepAlive 缓存已访问页面。
 * KeepAlive 只把组件移入缓存容器，**不触发 onUnmounted**，
 * 于是只在 onUnmounted 里清理的轮询会在切走后继续在后台跑（报文捕获 500ms 一次）。
 * 这里用真实的 <KeepAlive> + 动态组件验证：切走停表、切回补一次。
 */
describe('usePolling 在 KeepAlive 下的行为', () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    setVisibility('visible')
  })

  afterEach(() => {
    vi.useRealTimers()
    setVisibility('visible')
  })

  /**
   * 挂一个 KeepAlive 壳：默认渲染带轮询的 TabA，切到 TabB 即「切走」。
   * 返回切页用的 ref 与包装器。
   */
  function mountKeepAlive(fn: () => void | Promise<unknown>, interval = 500) {
    const current = ref('a')
    const TabA = defineComponent({
      setup() {
        usePolling(fn, interval)
        return () => h('div', 'A')
      },
    })
    const TabB = defineComponent({ setup: () => () => h('div', 'B') })
    const Shell = defineComponent({
      setup() {
        return () =>
          h(KeepAlive, null, {
            default: () => h(current.value === 'a' ? TabA : TabB, { key: current.value }),
          })
      },
    })
    const wrapper = mount(Shell)
    return { wrapper, current }
  }

  it('KeepAlive 切走的组件停表，切回时补一次', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { wrapper, current } = mountKeepAlive(fn, 500)
    expect(fn).toHaveBeenCalledTimes(1) // 挂载即执行一次
    await flushPromises()

    // 切走：deactivated → 停表
    current.value = 'b'
    await nextTick()
    await flushPromises()

    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 切走后不再轮询

    // 切回：activated → 立即补一次
    current.value = 'a'
    await nextTick()
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2)

    // 切回后恢复按周期轮询
    vi.advanceTimersByTime(500)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(3)
    wrapper.unmount()
  })

  it('KeepAlive 内首次挂载不会重复触发请求', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const { wrapper } = mountKeepAlive(fn, 500)
    await flushPromises()
    await nextTick()
    // onMounted 已跑过一次；Vue 在 KeepAlive 内首次挂载也会触发 activated，
    // 若不做区分，这里会变成 2 次
    expect(fn).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('被切走的订阅者不参与全局周期计算（不再把全局表钉在 500ms）', async () => {
    const fastFn = vi.fn().mockResolvedValue(undefined)
    const slowFn = vi.fn().mockResolvedValue(undefined)
    const current = ref('a')
    const TabA = defineComponent({
      setup() {
        usePolling(fastFn, 500)
        return () => h('div', 'A')
      },
    })
    const TabB = defineComponent({
      setup() {
        usePolling(slowFn, 5000)
        return () => h('div', 'B')
      },
    })
    const Shell = defineComponent({
      setup() {
        return () =>
          h(KeepAlive, null, {
            default: () => h(current.value === 'a' ? TabA : TabB, { key: current.value }),
          })
      },
    })
    const wrapper = mount(Shell)
    await flushPromises()

    // 切走 500ms 的订阅者，只剩 5000ms 的订阅者：全局周期应放宽
    current.value = 'b'
    await nextTick()
    await flushPromises()

    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(fastFn).toHaveBeenCalledTimes(1) // 切走的组件不再被唤醒
    expect(slowFn).toHaveBeenCalledTimes(2)
    wrapper.unmount()
  })

  it('全局最小间隔：低于下限的 interval 被抬到 GLOBAL_MIN_INTERVAL_MS', async () => {
    const fn = vi.fn().mockResolvedValue(undefined)
    const setIntervalSpy = vi.spyOn(globalThis, 'setInterval')
    const { wrapper } = mountPolling(fn, 50) // 显式写 50ms，模拟"页面写死高频轮询"
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 挂载即执行一次

    // 全局定时器周期被抬到下限
    expect(setIntervalSpy.mock.calls[0]?.[1]).toBe(GLOBAL_MIN_INTERVAL_MS)

    vi.advanceTimersByTime(GLOBAL_MIN_INTERVAL_MS - 1)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(1) // 不足下限不触发

    vi.advanceTimersByTime(1)
    await flushPromises()
    expect(fn).toHaveBeenCalledTimes(2)
    wrapper.unmount()
    setIntervalSpy.mockRestore()
  })

  it('并发闸门：同时在飞的轮询不超过 MAX_CONCURRENT_POLLS，余下的下一拍补跑', async () => {
    // 并发计数与轮转起点是模块级状态，前面用例可能留着未完成的回调；
    // 重置模块拿一份干净实例，保证本用例从「零在飞」出发
    vi.resetModules()
    const fresh = await import('./usePolling')
    const limit = fresh.MAX_CONCURRENT_POLLS

    const releases: Array<() => void> = []
    // 先让回调挂住（占满额度），释放后改为立即完成（验证被挡下的订阅者下一拍能跑）
    let hang = true
    const fns = Array.from({ length: limit + 1 }, () =>
      vi.fn(() =>
        hang ? new Promise<void>((resolve) => releases.push(resolve)) : Promise.resolve(),
      ),
    )
    const Comp = defineComponent({
      setup() {
        for (const fn of fns) fresh.usePolling(fn, 1000)
        return () => h('div')
      },
    })
    const wrapper = mount(Comp)
    const started = () => fns.reduce((n, f) => n + f.mock.calls.length, 0)

    // 首个 tick 逐个 runSubscriber，最多 limit 个占满并发额度
    expect(started()).toBe(limit)
    expect(fns[limit]!.mock.calls.length).toBe(0)

    // 在飞回调仍挂着时再推进一拍：额度未释放，不会有新的回调被发起
    vi.advanceTimersByTime(1000)
    expect(started()).toBe(limit)

    // 释放额度后（回调改为立即完成），被挡下的订阅者最终一定能轮到自己
    hang = false
    releases.splice(0).forEach((resolve) => resolve())
    await flushPromises()
    for (let i = 0; i < limit + 2 && fns[limit]!.mock.calls.length === 0; i++) {
      vi.advanceTimersByTime(1000)
    }
    expect(fns[limit]!.mock.calls.length).toBeGreaterThan(0)
    wrapper.unmount()
  })
})
