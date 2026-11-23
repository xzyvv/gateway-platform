import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import DataBuffering from './DataBuffering.vue'

/**
 * DD-51：断连续传页的「容量淘汰线」必须来自后端 queue_thresholds.evict，
 * 不得写死 100000（那是背压阈值，真实淘汰线是 max_rows 的 90%）。
 */
const mockGetStatus = vi.fn()
vi.mock('@/api/status', () => ({ getStatus: () => mockGetStatus() }))

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function stateOf(wrapper: ReturnType<typeof mount>): any {
  // @ts-expect-error access internal setup state
  return wrapper.vm.$.setupState
}

describe('DataBuffering.vue — 淘汰线由后端下发（DD-51）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('使用后端下发的 evict 计算占比与文案', async () => {
    mockGetStatus.mockResolvedValue({
      queue_length: 450_000,
      online_devices: 2,
      cpu_usage: 10,
      mem_usage: 20,
      queue_thresholds: { warning: 10_000, error: 100_000, evict: 900_000 },
    })
    const wrapper = mount(DataBuffering)
    await flushPromises()

    expect(stateOf(wrapper).evictRows).toBe(900_000)
    expect(stateOf(wrapper).queuePercent).toBe(50)
    const text = wrapper.text()
    expect(text).toContain('900,000')
    expect(text).toContain('50%')
    expect(text).not.toContain('100,000（')
    wrapper.unmount()
  })

  it('后端未下发 evict 时显示「—」，不回落到 100000', async () => {
    mockGetStatus.mockResolvedValue({
      queue_length: 120_000,
      online_devices: 1,
      cpu_usage: 5,
      mem_usage: 0,
      queue_thresholds: { warning: 10_000, error: 100_000 },
    })
    const wrapper = mount(DataBuffering)
    await flushPromises()

    expect(stateOf(wrapper).evictRows).toBeNull()
    expect(wrapper.text()).toContain('淘汰线未下发')
    expect(wrapper.text()).not.toContain('100,000 条')
    wrapper.unmount()
  })

  it('达到后端淘汰线时提示将删除最旧数据', async () => {
    mockGetStatus.mockResolvedValue({
      queue_length: 900_000,
      online_devices: 1,
      cpu_usage: 5,
      mem_usage: 0,
      queue_thresholds: { warning: 10_000, error: 100_000, evict: 900_000 },
    })
    const wrapper = mount(DataBuffering)
    await flushPromises()

    expect(stateOf(wrapper).overEvictLine).toBe(true)
    expect(wrapper.text()).toContain('已达容量淘汰线')
    wrapper.unmount()
  })
})
