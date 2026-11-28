import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import DataCompression from './DataCompression.vue'

/**
 * DD-52：压缩率必须来自后端真实统计（采集成功量 → 实际落库量），
 * 旧实现用配置比例 × 魔数 0.6/0.5 编造存储量与压缩率。
 */
const mockCharts: { setOption: ReturnType<typeof vi.fn>; dispose: ReturnType<typeof vi.fn> }[] = []

vi.mock('@/utils/chart/echarts-extra', () => ({
  default: {},
  initChart: () => {
    const c = { setOption: vi.fn(), dispose: vi.fn() }
    mockCharts.push(c)
    return c
  },
  disposeChart: vi.fn((c: { dispose: () => void } | null) => c?.dispose?.()),
}))

const mockGetConfig = vi.fn()
vi.mock('@/api/uplink/config', () => ({ getConfig: () => mockGetConfig() }))

const mockLoadStats = vi.fn()
vi.mock('@/composables/data/useStatsReport', () => ({
  useStatsReport: () => ({ load: () => mockLoadStats() }),
}))

vi.mock('@/api/acquisition/points', () => ({ batchEditPoints: vi.fn() }))
vi.mock('@/utils/storage/gatewayStorage', () => ({ scopedGet: () => '', scopedSet: vi.fn() }))

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function stateOf(wrapper: ReturnType<typeof mount>): any {
  // @ts-expect-error access internal setup state
  return wrapper.vm.$.setupState
}

describe('DataCompression.vue — 真实压缩统计（DD-52）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockCharts.length = 0
    mockGetConfig.mockResolvedValue({
      points: [{ id: 'p1', on_change: true, deadband: 0.5 }],
    })
    mockLoadStats.mockResolvedValue({
      summary: { totalReads: 1200, totalDataPoints: 1000, totalStoredPoints: 400 },
      deviceStats: [
        { deviceName: 'D1', totalReads: 700, dataPoints: 600, storedPoints: 300 },
        { deviceName: 'D2', totalReads: 500, dataPoints: 400, storedPoints: 100 },
      ],
    })
  })

  it('压缩率与存储量取自后端统计，不按配置比例估算', async () => {
    const wrapper = mount(DataCompression)
    await flushPromises()
    await nextTick()

    const stat = stateOf(wrapper).stat
    expect(stat.rawPoints).toBe(1000) // 采集成功点（不是含失败的总采集次数）
    expect(stat.storedPoints).toBe(400) // 后端实际落库样本数
    expect(stat.ratio).toBe(60) // 1 - 400/1000
    expect(wrapper.text()).toContain('60%')
    wrapper.unmount()
  })

  it('按设备对比使用后端 storedPoints / dataPoints', async () => {
    const wrapper = mount(DataCompression)
    await flushPromises()
    await nextTick()

    const option = mockCharts.at(-1)!.setOption.mock.calls[0]![0] as {
      series: { name: string; data: number[] }[]
    }
    const collected = option.series.find((s) => s.name === '采集成功点')
    const stored = option.series.find((s) => s.name === '存储数据点')
    expect(collected!.data).toEqual([600, 400])
    expect(stored!.data).toEqual([300, 100])
    wrapper.unmount()
  })
})
