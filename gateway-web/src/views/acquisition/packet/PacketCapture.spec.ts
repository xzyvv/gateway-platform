import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h, ref, nextTick, KeepAlive } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import PacketCapture from './PacketCapture.vue'

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
