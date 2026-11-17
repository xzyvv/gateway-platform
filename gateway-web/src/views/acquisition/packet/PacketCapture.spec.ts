import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h, ref, nextTick, KeepAlive } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import PacketCapture from './PacketCapture.vue'
import FilterToolbar from '@/components/FilterToolbar.vue'

const mockGetPackets = vi.fn()
const mockStartCapture = vi.fn()
const mockStopCapture = vi.fn()
const mockClearPackets = vi.fn()

vi.mock('@/api/protocolTools', () => ({
  startCapture: (...args: unknown[]) => mockStartCapture(...args),
  stopCapture: () => mockStopCapture(),
  getPackets: (...args: unknown[]) => mockGetPackets(...args),
  clearPackets: () => mockClearPackets(),
  saveSession: vi.fn(),
}))

vi.mock('@/composables/acquisition/useDebugTargets', () => ({
  useDebugTargets: () => ({ busOptions: ref([]) }),
}))

vi.mock('@/utils/clipboard', () => ({ copyText: vi.fn() }))

// useQueryState 依赖 route.query 与 router.replace：筛选条件记忆走 URL
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), currentRoute: { value: { query: {} } } }),
  useRoute: () => ({ query: {}, path: '/acquisition/console' }),
}))

/**
 * 报文捕获页在协议调试台里被 KeepAlive 缓存。
 * KeepAlive 切走不触发 onUnmounted → 500ms 轮询会在后台继续跑。
 * 这里钉住：切 tab 后轮询停，切回时补一次。
 */
describe('PacketCapture.vue — KeepAlive 下不空转', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockGetPackets.mockResolvedValue([])
    mockClearPackets.mockResolvedValue(undefined)
    // 只伪造 setInterval/clearInterval，保留真实 setTimeout（flushPromises 依赖它）
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  /** 挂一个 KeepAlive 壳：默认渲染报文捕获，切到另一页即「切走」 */
  function mountInKeepAlive() {
    const current = ref('capture')
    const Other = defineComponent({ setup: () => () => h('div', 'other') })
    const Shell = defineComponent({
      setup() {
        return () =>
          h(KeepAlive, null, {
            default: () =>
              h(current.value === 'capture' ? PacketCapture : Other, {
                key: current.value,
              }),
          })
      },
    })
    const wrapper = mount(Shell)
    return { wrapper, current }
  }

  it('切 tab 后抓包轮询停止，切回时补一次', async () => {
    const { wrapper, current } = mountInKeepAlive()
    await flushPromises()
    const base = mockGetPackets.mock.calls.length

    // 点击「开始抓包」→ 轮询启动
    const buttons = wrapper.findAll('button')
    const startBtn = buttons.find((b) => b.text().includes('开始抓包'))
    expect(startBtn).toBeTruthy()
    await startBtn!.trigger('click')
    await flushPromises()

    vi.advanceTimersByTime(500)
    await flushPromises()
    expect(mockGetPackets.mock.calls.length).toBeGreaterThan(base)

    // 切走：deactivated → 停表
    const afterStart = mockGetPackets.mock.calls.length
    current.value = 'other'
    await nextTick()
    await flushPromises()

    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(mockGetPackets.mock.calls.length).toBe(afterStart) // 切走后不再拉取

    // 切回：activated → 补一次
    current.value = 'capture'
    await nextTick()
    await flushPromises()
    expect(mockGetPackets.mock.calls.length).toBe(afterStart + 1)
    wrapper.unmount()
  })
})

/**
 * 加载失败与空态的可辨识性。
 *
 * 原实现 `load()` 没有 catch，首屏失败变成 unhandled rejection，表格只剩「暂无报文」——
 * 现场无法区分「接口挂了」「还没点开始抓包」「链路上真没报文」「被筛选筛掉了」，
 * 四种情况四种下一步动作，全被压成同一句话。
 */
describe('PacketCapture.vue — 失败与空态可辨识', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockClearPackets.mockResolvedValue(undefined)
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  const PACKET = {
    id: 'p1',
    direction: 'request' as const,
    time: 1_700_000_000_000,
    deviceName: '温控器',
    bus: 'COM1',
    deviceId: 'dev-1',
    delayMs: 12,
    hex: '01 03 00 00 00 01',
    parsed: '读保持寄存器',
  }

  it('首屏加载失败：出错误条与重试按钮，不再显示「暂无报文」', async () => {
    mockGetPackets.mockRejectedValue(new Error('gateway down'))
    const wrapper = mount(PacketCapture)
    await flushPromises()

    expect(wrapper.find('.table-error').exists()).toBe(true)
    expect(wrapper.text()).toContain('gateway down')
    // 失败不能被渲染成「还没开始抓包」：那会把用户引去点一个必然失败的按钮
    expect(wrapper.text()).not.toContain('尚未开始抓包')
    wrapper.unmount()
  })

  it('点「重试」重新拉取，成功后错误条消失且表格有数据', async () => {
    mockGetPackets.mockRejectedValue(new Error('gateway down'))
    const wrapper = mount(PacketCapture)
    await flushPromises()

    mockGetPackets.mockResolvedValue([PACKET])
    const retry = wrapper.findAll('button').find((b) => b.text().trim() === '重试')
    expect(retry, '错误条里应有重试按钮').toBeTruthy()
    await retry!.trigger('click')
    await flushPromises()

    expect(wrapper.find('.table-error').exists()).toBe(false)
    expect(wrapper.html()).toContain('01 03 00 00 00 01')
    wrapper.unmount()
  })

  it('拉取期间先画表格骨架，避免把「正在加载」显示成「没有报文」', async () => {
    let release: (() => void) | null = null
    mockGetPackets.mockImplementation(
      () => new Promise((resolve) => { release = () => resolve([] as never) }),
    )
    const wrapper = mount(PacketCapture)
    await flushPromises()

    // 表格处于 loading 时 DataTable 渲染骨架屏而不是空态文案
    expect(wrapper.find('.table-skeleton').exists()).toBe(true)
    expect(wrapper.text()).not.toContain('尚未开始抓包')

    release!()
    await flushPromises()
    expect(wrapper.find('.table-skeleton').exists()).toBe(false)
    expect(wrapper.text()).toContain('尚未开始抓包')
    wrapper.unmount()
  })

  it('空态区分「未开始抓包 / 已抓包无报文 / 被筛掉」', async () => {
    mockGetPackets.mockResolvedValue([])
    const wrapper = mount(PacketCapture)
    await flushPromises()

    // 1. 还没开始抓包：告诉用户下一步做什么
    expect(wrapper.text()).toContain('尚未开始抓包')

    // 2. 已开始抓包、总线上确实没有报文：该去查设备而不是再点按钮
    const startBtn = wrapper.findAll('button').find((b) => b.text().includes('开始抓包'))
    await startBtn!.trigger('click')
    await flushPromises()
    expect(wrapper.text()).toContain('当前总线上还没有报文往来')

    // 3. 有报文但被筛选条件筛掉：该去改筛选
    mockGetPackets.mockResolvedValue([PACKET])
    vi.advanceTimersByTime(500)
    await flushPromises()
    wrapper.findComponent(FilterToolbar).vm.$emit('update:modelValue', { bus: 'COM9', device: '' })
    await nextTick()
    expect(wrapper.text()).toContain('当前筛选条件下没有报文')
    wrapper.unmount()
  })
})
