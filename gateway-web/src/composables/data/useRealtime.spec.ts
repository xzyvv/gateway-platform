import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'

/** 捕获 useRealtime 注册到共享连接上的推送回调，用于直接触发真实 onMessage 路径 */
const wsMock = vi.hoisted(() => ({
  listener: null as null | ((data: unknown[]) => void),
}))

vi.mock('@/utils/websocket', () => ({
  GatewayWebSocket: class { constructor() { /* 占位：仅满足类型 */ } },
  acquireSharedWebSocket: () => ({
    ws: {
      connect: vi.fn(),
      close: vi.fn(),
      addMessageListener: vi.fn((cb: (data: unknown[]) => void) => {
        wsMock.listener = cb
        return () => { wsMock.listener = null }
      }),
    },
    release: vi.fn(),
  }),
}))

/** 图表实例桩：setOption 调用参数可断言（BUG-K 回归护栏） */
const chartMock = vi.hoisted(() => ({ setOption: vi.fn() }))

vi.mock('@/utils/echarts', () => ({
  // updateChartStructure 用 echarts.graphic.LinearGradient 构造面积填充，
  // 默认导出必须是带 graphic 的对象（`{}` 会在渲染路径上直接抛出 TypeError）
  default: { graphic: { LinearGradient: class { constructor(..._args: unknown[]) { /* 占位 */ } } } },
  initChart: () => ({
    setOption: chartMock.setOption,
    getOption: vi.fn(() => ({})),
    dispose: vi.fn(),
    resize: vi.fn(),
  }),
  disposeChart: vi.fn(),
  APPEND_THROTTLE_MS: 100,
}))

vi.mock('@/composables/core/useFavorites', () => ({
  useFavorites: () => ({ isFavorite: () => false, toggleFavorite: vi.fn() }),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

import { useRealtime } from './useRealtime'
// compactAxis 已随 W3b-13 拆到 utils/realtimeFormat（纯函数，与 WS/图表无关）
import { compactAxis } from '@/utils/realtimeFormat'
import type { DataPoint } from '@/api/types'

/**
 * Realtime 深层响应改造的回归护栏。
 *
 * 背景：pointList 已从 ref 改为 shallowRef、historyMap 已从 reactive(Map) 改为
 * 普通 Map。两处都必须有显式的 triggerRef / 版本号，否则表现是"表格不再更新"
 * 或"时间跨度标签永远停在旧值"——不报错、只在真实推送时才暴露。
 */
type Cli = ReturnType<typeof useRealtime>

const mounted: Array<{ unmount: () => void }> = []

/** 挂载并返回 composable API */
async function harness(): Promise<Cli> {
  let api!: Cli
  const Comp = defineComponent({
    setup() {
      api = useRealtime()
      // 必须把 chartRef 绑到真实节点：mountChart 在 chartRef 为空时直接 return，
      // 不绑定的话图表永远不会初始化，BUG-K 回归护栏会假红（setOption 一次都没调）。
      return () => h('div', { ref: api.chartRef })
    },
  })
  const wrapper = mount(Comp, { global: { plugins: [createPinia()] } })
  mounted.push(wrapper)
  await flushPromises()
  await nextTick()
  return api
}

/** 构造一条数据点 */
function dp(pointId: string, value: number, ts: number): DataPoint {
  return {
    point_id: pointId,
    device_id: 'dev1',
    value,
    raw_value: value,
    raw_type: 'u16',
    quality: 'good',
    timestamp: ts,
  }
}

describe('useRealtime 响应式契约', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    chartMock.setOption.mockReset()
    vi.useFakeTimers()
  })

  afterEach(() => {
    for (const w of mounted.splice(0)) w.unmount()
    vi.useRealTimers()
  })

  it('收到推送后 displayPoints / chartTimeSpan 均随之更新（shallowRef 需 triggerRef）', async () => {
    const api = await harness()
    expect(wsMock.listener).toBeTruthy()

    // computed 是惰性的：必须先读一次建立缓存，否则后续读到的永远是"最新值"，
    // 测试就会假绿——漏掉 triggerRef 也检测不出来。
    expect(api.displayPoints.value).toHaveLength(0)
    expect(api.chartTimeSpan.value).toBe('等待数据')

    // 走真实 onMessage：漏掉 triggerRef 时 displayPoints 不会重算
    wsMock.listener!([dp('p1', 1, 1000)])
    expect(api.displayPoints.value).toHaveLength(1)
    expect(api.displayPoints.value[0]!.value).toBe(1)

    // 时间跨度：依赖 historyVersion，漏掉自增则永远停在「等待数据」
    // 注意 chartTimeSpan 直接用原始 timestamp 求差（不换算单位），
    // 故这里用同一量纲的数值：1000 → 1120，差 120 即 120 秒
    wsMock.listener!([dp('p1', 2, 1120)])
    expect(api.chartTimeSpan.value).toBe('最近 2 分钟')

    // 同点更新走下标赋值分支，_flash 标记正确
    expect(api.displayPoints.value[0]!.value).toBe(2)
  })

  /**
   * BUG-K 回归护栏。
   *
   * 原实现走 ECharts appendData 增量刷新，但 series 是内联 data 数组，
   * appendData 必抛 TypeError 且被降级为 warn → 趋势图永久空白。
   * 这里钉住：推送后节流窗口必须真正 setOption 出 series；渲染抛错必须浮到 chartError。
   */
  it('BUG-K 回归：推送后 100ms 内重绘出 series；渲染失败暴露为 chartError', async () => {
    const api = await harness()
    wsMock.listener!([dp('p1', 1, 1000)])
    wsMock.listener!([dp('p1', 2, 1001)])
    await vi.advanceTimersByTimeAsync(100)

    const calls = chartMock.setOption.mock.calls
    const last = calls[calls.length - 1]![0] as {
      series: { name: string; data: unknown[] }[]
      xAxis: { data: unknown[] }
    }
    expect(last.series).toHaveLength(1)
    expect(last.series[0]!.name).toBe('p1')
    expect(last.series[0]!.data).toHaveLength(2)
    expect(last.xAxis.data.length).toBeGreaterThan(0)
    expect(api.chartError.value).toBeNull()

    // 渲染抛错必须可见（此前只有一条 warn 日志）
    chartMock.setOption.mockImplementationOnce(() => { throw new Error('boom') })
    wsMock.listener!([dp('p1', 3, 1002)])
    await vi.advanceTimersByTimeAsync(100)
    expect(api.chartError.value).toBe('boom')

    // 恢复后错误自动清空
    wsMock.listener!([dp('p1', 4, 1003)])
    await vi.advanceTimersByTimeAsync(100)
    expect(api.chartError.value).toBeNull()
  })

  it('flash 高亮在 800ms 后被清除', async () => {
    const api = await harness()
    wsMock.listener!([dp('p1', 1, 1000)])
    expect(api.pointList.value[0]!._flash).toBe(true)

    await vi.advanceTimersByTimeAsync(800)
    expect(api.pointList.value[0]!._flash).toBe(false)
  })

  it('格式与分类函数保持可用（qualityClass / formatValue / formatTime）', async () => {
    const api = await harness()
    expect(api.qualityClass('good')).toBe('quality-good')
    expect(api.qualityClass('bad')).toBe('quality-bad')
    expect(api.formatValue(1.2345)).toBe('1.234')
    expect(api.formatValue(null)).toBe('-')
    // 非有限值不能显示成 "NaN"/"Infinity"（现场会误当成正常读数），统一显示破折号
    expect(api.formatValue(Number.NaN)).toBe('—')
    expect(api.formatValue(Number.POSITIVE_INFINITY)).toBe('—')
    expect(api.formatValue(Number.NEGATIVE_INFINITY)).toBe('—')
    expect(api.formatValue('NaN')).toBe('—')
    // 非数字字符串是枚举原始值（ON/OPEN…），必须原样保留
    expect(api.formatValue('ON')).toBe('ON')
  })

  it('compactAxis：大数缩写，避免坐标轴撑开左边距', () => {
    // 五位以上数字逐位展示会把绘图区压扁，故万/亿缩写
    expect(compactAxis(9999)).toBe('9999')
    expect(compactAxis(15000)).toBe('1.5万')
    expect(compactAxis(150000000)).toBe('1.5亿')
    expect(compactAxis(0)).toBe('0')
    expect(compactAxis(1.234)).toBe('1.23')
    // 非有限值不能显示 "NaN" 在坐标轴上
    expect(compactAxis(NaN)).toBe('')
    expect(compactAxis(Infinity)).toBe('')
  })

  it('初始状态：空列表、空态文案、无图表报错', async () => {
    const api = await harness()
    expect(api.pointList.value).toEqual([])
    expect(api.displayPoints.value).toEqual([])
    expect(api.emptyText.value).toBeTruthy()
  })

  /**
   * N4：空态必须按原因分区。
   *
   * 原实现只有一句「已连接，等待首帧采集数据…」。当用户勾选「仅看收藏」而一个都没收藏时，
   * 表格是空的但实时数据其实一直在推送——同一句文案会让人误以为网关没采到数据。
   */
  it('N4 空态分区：仅看收藏过滤为空要说「筛选」而不是「等待首帧」', async () => {
    const api = await harness()
    // 真正收到首帧后才有 pointList，此时开启「仅看收藏」(0 个收藏) → 应为筛选态
    wsMock.listener!([dp('p1', 1, 1000)])
    api.onlyFavorites.value = true
    expect(api.pointList.value.length).toBe(1)
    expect(api.displayPoints.value).toEqual([])
    expect(api.emptyKind.value).toBe('filtered')
    expect(api.emptyText.value).toContain('筛选')
    expect(api.emptyActionText.value).toBe('显示全部点位')
  })

  it('冻结期间表格不更新，恢复后追平到最新帧', async () => {
    const api = await harness()
    wsMock.listener!([dp('p1', 1, 1000)])
    expect(api.displayPoints.value[0]!.value).toBe(1)

    api.togglePause()
    expect(api.paused.value).toBe(true)

    // 冻结期间推两帧：表格必须停在冻结前的值
    wsMock.listener!([dp('p1', 2, 1001)])
    wsMock.listener!([dp('p1', 3, 1002)])
    expect(api.displayPoints.value[0]!.value).toBe(1)

    // 恢复后只回放最新一帧（中间帧只进历史，不逐帧闪表格）
    api.togglePause()
    expect(api.paused.value).toBe(false)
    expect(api.displayPoints.value[0]!.value).toBe(3)
  })

  it('突变标记：变化 ≥50% 打标，值停住时保留，回归正常波动时清除', async () => {
    const api = await harness()
    wsMock.listener!([dp('p1', 100, 1000)])
    expect(api.displayPoints.value[0]!._surge).toBeUndefined()

    // 100 → 200（+100%）：打标 up
    wsMock.listener!([dp('p1', 200, 1001)])
    expect(api.displayPoints.value[0]!._surge).toBe('up')
    expect(api.displayPoints.value[0]!._surgePct).toBe(100)

    // 值停住（200 → 200）：标记保留，不能闪一下就消失
    wsMock.listener!([dp('p1', 200, 1002)])
    expect(api.displayPoints.value[0]!._surge).toBe('up')

    // 回归正常波动（200 → 204，+2%）：标记清除
    wsMock.listener!([dp('p1', 204, 1003)])
    expect(api.displayPoints.value[0]!._surge).toBeUndefined()

    // 向下突变：204 → 50（约 -75.5%）：打标 down
    wsMock.listener!([dp('p1', 50, 1004)])
    expect(api.displayPoints.value[0]!._surge).toBe('down')
  })

  /**
   * 时间跨度标签只统计**图表实际绘制的 top5**。
   *
   * 背景：图表本来只画 top5，但原先 chartTimeSpan 遍历全部点位的历史
   * （1200 点位 × 60 条 = 7.2 万次比较/批）。这里钉住两件事：
   *   1. 口径正确：跨度取自图表里那些点位的最早/最晚时间；
   *   2. top5 之外的历史不参与计算（否则「数据跨度」会被看不见的曲线拉长）。
   */
  it('chartTimeSpan 只统计图表 top5，top5 之外的历史不影响跨度', async () => {
    const api = await harness()
    expect(api.chartTimeSpan.value).toBe('等待数据')

    // 前 5 个点位（图表会绘制）：跨度 = 1120 - 1000 = 120 秒
    for (let i = 1; i <= 5; i++) {
      wsMock.listener!([dp(`p${i}`, i, 1000)])
    }
    wsMock.listener!([dp('p1', 2, 1120)])
    expect(api.chartTimeSpan.value).toBe('最近 2 分钟')

    // 第 6 个及之后的点位不参与图表（top5 已满），它的时间戳不该拉长跨度
    wsMock.listener!([dp('p6', 6, 99999)])
    expect(api.chartTimeSpan.value).toBe('最近 2 分钟')
  })

  it('chartTimeSpan：只有 1 个点位且时间跨度为 0 时回落到「等待数据」', async () => {
    const api = await harness()
    wsMock.listener!([dp('p1', 1, 1000)])
    // 单点 → max == min → 无跨度可言
    expect(api.chartTimeSpan.value).toBe('等待数据')

    wsMock.listener!([dp('p1', 2, 1001)])
    expect(api.chartTimeSpan.value).toBe('最近 1 秒')
  })
})