import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount } from '@vue/test-utils'
import { useUnmountedGuard } from './useUnmountedGuard'

/**
 * 契约：卸载守卫是「组件已卸载就别再往里写状态」的唯一实现。
 *
 * 它被 14 处调用方依赖（8 个页面 + 4 个 composable + 2 个 state.ts），
 * 一旦行为漂移，表现为「卸载后写状态」这类偶发难查的问题。
 */
describe('useUnmountedGuard', () => {
  it('组件存活期间 isUnmounted() 为 false', () => {
    let result: (() => boolean) | null = null
    const Host = defineComponent({
      setup() {
        const { isUnmounted } = useUnmountedGuard()
        result = isUnmounted
        return () => h('div')
      },
    })
    mount(Host)
    expect(result!()).toBe(false)
  })

  it('组件卸载后 isUnmounted() 变为 true', () => {
    let result: (() => boolean) | null = null
    const Host = defineComponent({
      setup() {
        const { isUnmounted } = useUnmountedGuard()
        result = isUnmounted
        return () => h('div')
      },
    })
    const wrapper = mount(Host)
    expect(result!()).toBe(false)
    wrapper.unmount()
    expect(result!()).toBe(true)
  })

  it('无宿主实例时调用不产生 Vue 生命周期警告（可被普通函数 / 单测直接复用）', () => {
    // 关键：无 getCurrentInstance 时不能注册 onUnmounted，否则 Vue 会警告
    // "onUnmounted is called when there is no active component instance"。
    // 这类警告在单测里是噪音，在真实代码里会掩盖真正的告警。
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    try {
      const { isUnmounted } = useUnmountedGuard()
      expect(isUnmounted()).toBe(false)
      const lifecycleWarnings = warn.mock.calls.filter((c) =>
        String(c[0]).includes('onUnmounted is called when there is no active component instance'),
      )
      expect(lifecycleWarnings).toEqual([])
    } finally {
      warn.mockRestore()
    }
  })

  it('多次调用互不影响（各自独立实例）', () => {
    const a = useUnmountedGuard()
    const b = useUnmountedGuard()
    expect(a.isUnmounted()).toBe(false)
    expect(b.isUnmounted()).toBe(false)
  })
})
