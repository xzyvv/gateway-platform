import { describe, it, expect } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount } from '@vue/test-utils'
import { useRequestGuard } from './useRequestGuard'

/**
 * useRequestGuard：只接受「最新一次」请求的结果。
 *
 * 覆盖三类作废场景：
 *   1. 发了新请求 → 旧请求的结果被丢弃
 *   2. 组件卸载 → 所有在途请求的结果被丢弃
 *   3. 同一轮内的多次调用共享同一个序号（isCurrent 仍为 true）
 */
describe('useRequestGuard', () => {
  it('新一轮请求作废上一轮的结果', () => {
    const { next, isCurrent } = useRequestGuard()
    const first = next()
    expect(isCurrent(first)).toBe(true)

    const second = next()
    // 第一轮已过期
    expect(isCurrent(first)).toBe(false)
    // 第二轮是当前轮
    expect(isCurrent(second)).toBe(true)
  })

  it('序号严格递增', () => {
    const { next } = useRequestGuard()
    expect(next()).toBe(1)
    expect(next()).toBe(2)
    expect(next()).toBe(3)
  })

  it('组件卸载后所有在途请求的结果被丢弃', () => {
    const wrapper = mount(defineComponent({
      setup() {
        const { next, isCurrent } = useRequestGuard()
        return { next, isCurrent }
      },
      render: () => h('div'),
    }))

    // @ts-expect-error access internal setup state
    const { next, isCurrent } = wrapper.vm.$.setupState
    const seq = next()
    expect(isCurrent(seq)).toBe(true)

    wrapper.unmount()
    // 卸载后 isCurrent 恒为 false：在途响应不会再写进已销毁的组件
    expect(isCurrent(seq)).toBe(false)
  })

  it('卸载后再取号仍会得到一个已作废的序号', () => {
    const wrapper = mount(defineComponent({
      setup() {
        const { next, isCurrent } = useRequestGuard()
        return { next, isCurrent }
      },
      render: () => h('div'),
    }))

    // @ts-expect-error access internal setup state
    const { next, isCurrent } = wrapper.vm.$.setupState
    wrapper.unmount()

    const seq = next()
    expect(isCurrent(seq)).toBe(false)
  })
})
