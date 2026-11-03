import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createMemoryHistory, createRouter } from 'vue-router'
import LoadFailedRoute from './LoadFailedRoute.vue'

/** LoadFailedRoute 测试
 *
 * 注：error 触发路由由 router.onError 完成（集成测试覆盖），这里只覆盖组件层契约：
 *   - meta 透传错误信息正确渲染
 *   - 重试 / 返回首页 / 刷新整页 / 复制错误 4 个按钮 emit/调用正确
 *   - 上报 reporter
 */

// mock useErrorReporter 让测试可验证上报调用
const mockReportPageError = vi.fn()
vi.mock('../composables/core/useErrorReporter', () => ({
  reportPageError: (...args: unknown[]) => mockReportPageError(...args),
}))

// mock 整页导航封装：vmThreads 池下 window.location.assign/reload 不可重定义
// （Cannot redefine property），无法直接 spyOn；经 utils/navigation 间接层后
// 用模块级 mock 断言。forks 池时期的 defineProperty(window,'location') 写法已不可用。
const mockNavigateTo = vi.fn()
const mockReloadPage = vi.fn()
vi.mock('@/utils/navigation', () => ({
  navigateTo: (...args: unknown[]) => mockNavigateTo(...args),
  reloadPage: () => mockReloadPage(),
}))

async function setup(opts: { fromQuery?: string; loadError?: string } = {}) {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      {
        path: '/load-failed',
        name: 'LoadFailed',
        component: LoadFailedRoute,
      },
      { path: '/overview', component: { template: '<div />' } },
    ],
  })
  const query: Record<string, string> = {}
  if (opts.fromQuery) query.from = opts.fromQuery
  router.push({ name: 'LoadFailed', query })
  await router.isReady()
  const wrapper = mount(LoadFailedRoute, {
    global: { plugins: [router] },
  })
  await flushPromises()
  return { wrapper, router }
}

describe('LoadFailedRoute', () => {
  beforeEach(() => {
    mockReportPageError.mockClear()
    mockNavigateTo.mockClear()
    mockReloadPage.mockClear()
  })

  it('渲染降级标题与默认错误信息', async () => {
    const { wrapper } = await setup()
    expect(wrapper.find('.load-failed-card').exists()).toBe(true)
    expect(wrapper.text()).toContain('页面加载失败')
    // 默认错误文案：meta 没有时显示"未知加载错误"
    expect(wrapper.text()).toContain('未知加载错误')
  })

  it('从 query.from 读取来源路径', async () => {
    const { wrapper } = await setup({ fromQuery: '/devices' })
    const detail = wrapper.find('.load-failed-detail pre')
    expect(detail.exists()).toBe(true)
    expect(detail.text()).toContain('来源路径：/devices')
  })

  it('点击"重试加载"会整页跳转回来源路径（仅来源路径非空时）', async () => {
    const { wrapper } = await setup({ fromQuery: '/devices' })
    const btn = wrapper.findAll('button').find((b) => b.text().includes('重试加载'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(mockNavigateTo).toHaveBeenCalledWith('/devices')
  })

  it('无 fromQuery 时点击"重试加载"降级为整页刷新', async () => {
    const { wrapper } = await setup()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('重试加载'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(mockReloadPage).toHaveBeenCalled()
  })

  it('点击"返回首页"调 router.replace(/overview)', async () => {
    const { wrapper, router } = await setup()
    const replaceSpy = vi.spyOn(router, 'replace')
    const btn = wrapper.findAll('button').find((b) => b.text().includes('返回首页'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(replaceSpy).toHaveBeenCalledWith('/overview')
  })

  it('点击"刷新整页"调整页刷新', async () => {
    const { wrapper } = await setup()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('刷新整页'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(mockReloadPage).toHaveBeenCalled()
  })

  it('点击"复制错误"调 navigator.clipboard.writeText', async () => {
    const { wrapper } = await setup({ fromQuery: '/history' })
    const writeSpy = vi.fn().mockResolvedValue(undefined)
    // navigator.clipboard 为只读属性，用 defineProperty 注入 mock（无需类型逃逸）
    Object.defineProperty(globalThis.navigator, 'clipboard', {
      value: { writeText: writeSpy },
      configurable: true,
    })
    const btn = wrapper.findAll('button').find((b) => b.text().includes('复制错误'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()
    expect(writeSpy).toHaveBeenCalled()
    expect(writeSpy.mock.calls[0]![0]).toContain('来源路径：/history')
  })

  it('组件挂载时调用 reportPageError', async () => {
    await setup({ fromQuery: '/devices' })
    expect(mockReportPageError).toHaveBeenCalledTimes(1)
    const payload = mockReportPageError.mock.calls[0]![0]
    expect(payload.boundary).toBe('route-load')
    expect(payload.name).toContain('/load-failed')
    expect(payload.info.kind).toBe('dynamic-import')
  })
})