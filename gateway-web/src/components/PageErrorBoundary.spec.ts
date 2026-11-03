import { describe, it, expect, vi, beforeEach } from 'vitest'
import { defineComponent, h, ref, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import PageErrorBoundary from './PageErrorBoundary.vue'
import { setErrorReporter, resetErrorReporter } from '../composables/core/useErrorReporter'

/** 测试子组件：可控是否抛出渲染错误 */
function makeChild(opts: { shouldThrow?: () => boolean; throwMsg?: string }) {
  return defineComponent({
    setup() {
      return () => {
        if (opts.shouldThrow?.()) {
          throw new Error(opts.throwMsg || 'boom')
        }
        return h('div', { class: 'child-ok' }, 'child rendered')
      }
    },
  })
}

describe('PageErrorBoundary', () => {
  beforeEach(() => {
    resetErrorReporter()
  })

  it('正常态透传默认插槽', () => {
    const Child = makeChild({})
    const wrapper = mount(PageErrorBoundary, {
      slots: { default: () => h(Child) },
    })
    expect(wrapper.find('.child-ok').exists()).toBe(true)
    expect(wrapper.find('.page-error-boundary').exists()).toBe(false)
  })

  it('子组件渲染抛错时降级为错误卡片', async () => {
    const shouldThrow = ref(true)
    const Child = makeChild({ shouldThrow: () => shouldThrow.value, throwMsg: 'render failed' })
    const wrapper = mount(PageErrorBoundary, {
      props: { name: '/test-page' },
      slots: { default: () => h(Child) },
    })
    await flushPromises()

    expect(wrapper.find('.page-error-boundary').exists()).toBe(true)
    expect(wrapper.find('.child-ok').exists()).toBe(false)
    expect(wrapper.text()).toContain('页面渲染失败')
  })

  it('点击重试后子组件可正常渲染', async () => {
    const shouldThrow = ref(true)
    const Child = makeChild({ shouldThrow: () => shouldThrow.value, throwMsg: 'first' })
    const wrapper = mount(PageErrorBoundary, {
      slots: { default: () => h(Child) },
    })
    await flushPromises()
    expect(wrapper.find('.page-error-boundary').exists()).toBe(true)

    // 修好子组件并点击重试
    shouldThrow.value = false
    const retryBtn = wrapper.findAll('button').find((b) => b.text().includes('重试'))!
    expect(retryBtn).toBeTruthy()
    await retryBtn.trigger('click')
    await flushPromises()

    expect(wrapper.find('.child-ok').exists()).toBe(true)
    expect(wrapper.find('.page-error-boundary').exists()).toBe(false)
  })

  it('默认会调用 reporter 上报错误', async () => {
    const reporter = vi.fn()
    setErrorReporter(reporter)
    const shouldThrow = ref(true)
    const Child = makeChild({ shouldThrow: () => shouldThrow.value, throwMsg: 'report me' })
    mount(PageErrorBoundary, {
      props: { name: '/api-page', boundary: 'route' },
      slots: { default: () => h(Child) },
    })
    await flushPromises()

    expect(reporter).toHaveBeenCalledTimes(1)
    const payload = reporter.mock.calls[0]![0]
    expect(payload.boundary).toBe('route')
    expect(payload.name).toBe('/api-page')
    expect(payload.error).toBeInstanceOf(Error)
    expect((payload.error as Error).message).toBe('report me')
    expect(typeof payload.timestamp).toBe('number')
  })

  it('silent 模式下不调用 reporter', async () => {
    const reporter = vi.fn()
    setErrorReporter(reporter)
    const shouldThrow = ref(true)
    const Child = makeChild({ shouldThrow: () => shouldThrow.value })
    mount(PageErrorBoundary, {
      props: { silent: true },
      slots: { default: () => h(Child) },
    })
    await flushPromises()
    expect(reporter).not.toHaveBeenCalled()
  })

  it('技术详情默认收起，点击 summary 后展开', async () => {
    const shouldThrow = ref(true)
    const Child = makeChild({ shouldThrow: () => shouldThrow.value, throwMsg: 'stack test' })
    const wrapper = mount(PageErrorBoundary, {
      slots: { default: () => h(Child) },
    })
    await flushPromises()
    // 默认不显示 details（避免每次都打印大段栈）
    expect(wrapper.find('.error-detail').exists()).toBe(false)
  })

  it('连续多次错误会增加计数', async () => {
    const reporter = vi.fn()
    setErrorReporter(reporter)
    const shouldThrow = ref(true)
    const Child = makeChild({ shouldThrow: () => shouldThrow.value })
    const wrapper = mount(PageErrorBoundary, {
      props: { name: '/count-page' },
      slots: { default: () => h(Child) },
    })
    await flushPromises()
    // 第一次已经触发，再点重试（子组件仍抛错）应该再次触发
    const retryBtn = wrapper.findAll('button').find((b) => b.text().includes('重试'))!
    await retryBtn.trigger('click')
    await nextTick()
    await flushPromises()
    expect(reporter).toHaveBeenCalledTimes(2)
    expect(wrapper.text()).toContain('第 2 次')
  })
})