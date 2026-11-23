import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import DataCompare from './DataCompare.vue'

/**
 * DataCompare 的两个缺陷回归：
 *
 * 1. 图表宿主被 v-if 卸载后重新创建时，旧实例绑定的是已移除的 DOM，
 *    `if (!chart)` 会跳过重建 → setOption 写向已销毁的 DOM → 新宿主永久空白。
 * 2. 查询 await 期间用户改选择，`selectedPoints.value[j]!` 非空断言越界
 *    → TypeError: Cannot read properties of undefined (reading 'id')。
 */

// ---- echarts mock：每次 init 返回**新**实例（真实 echarts 行为）----
type MockChart = {
  setOption: ReturnType<typeof vi.fn>
  resize: ReturnType<typeof vi.fn>
  dispose: ReturnType<typeof vi.fn>
  getDom: ReturnType<typeof vi.fn<() => HTMLElement>>
  isDisposed: ReturnType<typeof vi.fn<() => boolean>>
}
const mockCharts: MockChart[] = []

function newMockChart(dom: HTMLElement): MockChart {
  const c: MockChart = {
    setOption: vi.fn(),
    resize: vi.fn(),
    dispose: vi.fn(),
    getDom: vi.fn(() => dom),
    isDisposed: vi.fn(() => false),
  }
  mockCharts.push(c)
  return c
}

vi.mock('@/utils/echarts', () => ({
  default: { init: () => mockCharts.at(-1), graphic: { LinearGradient: class {} } },
  initChart: (dom: HTMLElement) => newMockChart(dom),
  disposeChart: vi.fn((c: { dispose: () => void }) => c?.dispose?.()),
  ensureChart: (current: { getDom: () => HTMLElement; dispose: () => void } | null, dom: HTMLElement | null | undefined) => {
    if (!dom) {
      current?.dispose?.()
      return null
    }
    if (!current || current.getDom() !== dom) {
      current?.dispose?.()
      return newMockChart(dom)
    }
    return current
  },
}))

// ---- 数据源 mock ----
const mockFetchHistorySamples = vi.fn()
vi.mock('@/composables/data/useHistoryQuery', () => ({
  fetchHistorySamples: (opts: unknown) => mockFetchHistorySamples(opts),
}))

const mockListDevices = vi.fn()
vi.mock('@/api/devices', () => ({ listDevices: () => mockListDevices() }))

const mockListPoints = vi.fn()
vi.mock('@/api/points', () => ({ listPoints: (id: string) => mockListPoints(id) }))

const DEVICES = [{ id: 'd1', name: '仪表1' }]
const POINTS = [
  { id: 'p1', name: '温度' },
  { id: 'p2', name: '压力' },
]

/** 读取组件内部状态（script setup 无公开类型，沿用仓库既有 setupState 约定） */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function stateOf(wrapper: ReturnType<typeof mount>): any {
  // @ts-expect-error access internal setup state
  return wrapper.vm.$.setupState
}

/** 选中前 n 个点位（通过组件暴露的 selectedPointIds） */
async function selectPoints(wrapper: ReturnType<typeof mount>, ids: string[]) {
  stateOf(wrapper).selectedPointIds = ids
  await nextTick()
}

describe('DataCompare.vue — 图表重建与查询越界', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockCharts.length = 0
    mockListDevices.mockResolvedValue(DEVICES)
    mockListPoints.mockImplementation(async (id: string) => (id === 'd1' ? POINTS : []))
    mockFetchHistorySamples.mockResolvedValue({
      items: [{ timestamp: 1700000000000, value: 1 }],
      total: 1,
    })
  })

  it('加载点位列表并渲染选择器', async () => {
    const wrapper = mount(DataCompare)
    await flushPromises()
    await nextTick()

    expect(mockListDevices).toHaveBeenCalled()
    expect(stateOf(wrapper).allPoints.length).toBe(2)
    wrapper.unmount()
  })

  it('查询后渲染图表', async () => {
    const wrapper = mount(DataCompare)
    await flushPromises()
    await selectPoints(wrapper, ['p1'])

    const btn = wrapper.findAll('button').find((b) => b.text().includes('查询'))!
    await btn.trigger('click')
    await flushPromises()
    await nextTick()

    expect(mockFetchHistorySamples).toHaveBeenCalled()
    expect(mockCharts.length).toBe(1)
    expect(mockCharts[0]!.setOption).toHaveBeenCalled()
    wrapper.unmount()
  })

  /**
   * DD-53 回归：/api/history 截断时必须显式提示，不能把部分数据当全部。
   *
   * 原实现忽略 `truncated` / `has_more`，窗口内 5000 条只返回 2 条也无任何提示。
   */
  it('数据被截断时显示截断提示（DD-53）', async () => {
    mockFetchHistorySamples.mockResolvedValue({
      items: [
        { timestamp: 1700000000000, value: 1 },
        { timestamp: 1700000001000, value: 2 },
      ],
      total: 5000,
      returned: 2,
      truncated: true,
      has_more: true,
      granularity: 'raw',
    })
    const wrapper = mount(DataCompare)
    await flushPromises()
    await selectPoints(wrapper, ['p1'])

    const btn = wrapper.findAll('button').find((b) => b.text().includes('查询'))!
    await btn.trigger('click')
    await flushPromises()
    await nextTick()

    expect(stateOf(wrapper).truncation).toEqual({ point: '仪表1/温度', total: 5000, returned: 2 })
    const text = wrapper.text()
    expect(text).toContain('数据已截断')
    expect(text).toContain('5,000')
    wrapper.unmount()
  })

  it('未截断时不显示提示（DD-53）', async () => {
    const wrapper = mount(DataCompare)
    await flushPromises()
    await selectPoints(wrapper, ['p1'])

    const btn = wrapper.findAll('button').find((b) => b.text().includes('查询'))!
    await btn.trigger('click')
    await flushPromises()
    await nextTick()

    expect(stateOf(wrapper).truncation).toBeNull()
    expect(wrapper.text()).not.toContain('数据已截断')
    wrapper.unmount()
  })

  /**
   * 回归：查询期间改选择不越界。
   *
   * 原实现 await 之后仍读 computed 的 selectedPoints.value[j]!，
   * 期间把选中的 2 个点位减到 1 个 → j=1 时越界 → TypeError。
   */
  it('查询进行中改选择不抛越界错误（快照保护）', async () => {
    const wrapper = mount(DataCompare)
    await flushPromises()
    await selectPoints(wrapper, ['p1', 'p2'])

    // 让请求挂起，等我们在 await 期间改选择
    // 每个点位各发一个请求（Promise.all），逐个放行
    const resolvers: ((v: unknown) => void)[] = []
    mockFetchHistorySamples.mockImplementation(
      () => new Promise((r) => { resolvers.push(r) }),
    )

    const btn = wrapper.findAll('button').find((b) => b.text().includes('查询'))!
    await btn.trigger('click')
    await nextTick()

    // await 期间把选择从 2 个减到 1 个
    await selectPoints(wrapper, ['p1'])

    // 放行请求 → 原实现会在 selectedPoints.value[1]! 处越界
    for (const r of resolvers) r({ items: [{ timestamp: 1700000000000, value: 1 }], total: 1 })
    await flushPromises()
    await nextTick()

    expect(wrapper.exists()).toBe(true)
    // 表格按快照的 2 个点位生成列
    expect(stateOf(wrapper).dataRows.length).toBeGreaterThan(0)
    wrapper.unmount()
  })

  /**
   * 回归：宿主被 v-if 卸载后重建，图表实例必须跟着重建。
   *
   * 原实现 `if (!chart) chart = initChart(...)` —— 旧实例非空就跳过，
   * setOption 写向已移除的 DOM → 新宿主图表区永久空白。
   */
  it('清空选择后重新查询：图表实例随宿主重建', async () => {
    const wrapper = mount(DataCompare)
    await flushPromises()
    await selectPoints(wrapper, ['p1'])

    const btn = wrapper.findAll('button').find((b) => b.text().includes('查询'))!
    await btn.trigger('click')
    await flushPromises()
    await nextTick()

    const first = mockCharts.at(-1)!
    expect(first.setOption).toHaveBeenCalled()
    expect(mockCharts.length).toBe(1)

    // 清空选择 → v-if 卸载宿主 → 旧实例应被释放
    await selectPoints(wrapper, [])
    await nextTick()
    expect(first.dispose).toHaveBeenCalled()

    // 重新选择 + 查询 → 宿主重建 → 必须新建实例
    await selectPoints(wrapper, ['p1'])
    await btn.trigger('click')
    await flushPromises()
    await nextTick()

    expect(mockCharts.length).toBe(2)
    expect(mockCharts.at(-1)).not.toBe(first)
    expect(mockCharts.at(-1)!.setOption).toHaveBeenCalled()
    wrapper.unmount()
  })
})
