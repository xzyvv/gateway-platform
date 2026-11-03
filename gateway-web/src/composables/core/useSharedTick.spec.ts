import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount } from '@vue/test-utils'
import { useSharedTick, __resetSharedTickForTests, type UseSharedTickReturn } from './useSharedTick'

/** 在组件 setup 中挂载 useSharedTick，返回包装器与句柄 */
function mountTick(run: () => void, interval = 1000) {
  let api: UseSharedTickReturn | undefined
  const Comp = defineComponent({
    setup() {
      api = useSharedTick(run, interval)
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  return { wrapper, get api() { return api! } }
}

function setVisibility(state: 'visible' | 'hidden') {
  Object.defineProperty(document, 'visibilityState', { value: state, configurable: true })
  document.dispatchEvent(new Event('visibilitychange'))
}

describe('useSharedTick', () => {
  beforeEach(() => {
    // 只伪造本模块用到的定时器与 Date，保留真实 setTimeout（mount/flush 依赖它）
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval', 'Date'] })
    __resetSharedTickForTests()
    setVisibility('visible')
  })

  afterEach(() => {
    vi.useRealTimers()
    __resetSharedTickForTests()
    setVisibility('visible')
  })

  it('多个订阅者共享同一个定时器，全部按 1s 回调', () => {
    const a = vi.fn()
    const b = vi.fn()
    const wa = mountTick(a).wrapper
    const wb = mountTick(b).wrapper

    expect(vi.getTimerCount()).toBe(1)

    vi.advanceTimersByTime(1000)
    expect(a).toHaveBeenCalledTimes(1)
    expect(b).toHaveBeenCalledTimes(1)

    // 卸载全部订阅者后定时器必须归零，不留悬挂唤醒
    wa.unmount()
    wb.unmount()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('周期不同的订阅者各按自己的节奏回调', () => {
    const fast = vi.fn()
    const slow = vi.fn()
    const wf = mountTick(fast, 1000).wrapper
    const ws = mountTick(slow, 5000).wrapper

    vi.advanceTimersByTime(1000)
    expect(fast).toHaveBeenCalledTimes(1)
    expect(slow).not.toHaveBeenCalled()

    vi.advanceTimersByTime(4000)
    expect(fast).toHaveBeenCalledTimes(5)
    expect(slow).toHaveBeenCalledTimes(1)

    wf.unmount()
    ws.unmount()
  })

  it('setInterval 调整周期后按新节奏回调，且不影响其他订阅者', () => {
    const run = vi.fn()
    const other = vi.fn()
    const { wrapper, api } = mountTick(run, 1000)
    const otherWrapper = mountTick(other, 1000).wrapper

    vi.advanceTimersByTime(1000)
    expect(run).toHaveBeenCalledTimes(1)

    // 降频到 30s：此后 29s 内不再触发
    api.setInterval(30_000)
    vi.advanceTimersByTime(29_000)
    expect(run).toHaveBeenCalledTimes(1)
    // 另一个订阅者仍按 1s 走，未被降频影响
    expect(other).toHaveBeenCalledTimes(30)

    vi.advanceTimersByTime(1000)
    expect(run).toHaveBeenCalledTimes(2)

    wrapper.unmount()
    otherWrapper.unmount()
  })

  it('标签页隐藏时停表，回到前台立即补一次并恢复', () => {
    const run = vi.fn()
    const { wrapper } = mountTick(run)
    expect(vi.getTimerCount()).toBe(1)

    setVisibility('hidden')
    expect(vi.getTimerCount()).toBe(0)
    vi.advanceTimersByTime(10_000)
    expect(run).not.toHaveBeenCalled()

    setVisibility('visible')
    expect(run).toHaveBeenCalledTimes(1) // 立即补一次
    expect(vi.getTimerCount()).toBe(1)

    vi.advanceTimersByTime(1000)
    expect(run).toHaveBeenCalledTimes(2)

    wrapper.unmount()
  })

  it('单个回调抛错不影响其他订阅者', () => {
    const boom = vi.fn(() => { throw new Error('bad callback') })
    const ok = vi.fn()
    const w1 = mountTick(boom).wrapper
    const w2 = mountTick(ok).wrapper

    vi.advanceTimersByTime(1000)
    expect(boom).toHaveBeenCalledTimes(1)
    expect(ok).toHaveBeenCalledTimes(1)

    w1.unmount()
    w2.unmount()
  })

  it('start 幂等：重复调用不叠加订阅', () => {
    const run = vi.fn()
    const { wrapper, api } = mountTick(run)
    api.start()
    api.start()
    vi.advanceTimersByTime(1000)
    expect(run).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })
})
