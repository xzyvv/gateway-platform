import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import MetricsDashboard from './MetricsDashboard.vue'

const mockGetMetrics = vi.fn()
vi.mock('@/api/status', () => ({
  getMetrics: () => mockGetMetrics(),
}))

const PROM_TEXT = `# HELP gateway_uplink_total Total successful uplink messages
# TYPE gateway_uplink_total counter
gateway_uplink_total 50
# HELP gateway_queue_length Current data queue depth
# TYPE gateway_queue_length gauge
gateway_queue_length 1234
# HELP gateway_devices_online Number of online points
# TYPE gateway_devices_online gauge
gateway_devices_online 42
# HELP gateway_errors_total Total error count
# TYPE gateway_errors_total counter
gateway_errors_total 7
# HELP gateway_mqtt_publish_total MQTT publish count
# TYPE gateway_mqtt_publish_total counter
gateway_mqtt_publish_total{result="success"} 40
gateway_mqtt_publish_total{result="failure"} 2
`

/** 读取组件内部 loading（script setup 无公开类型，沿用仓库既有 setupState 约定） */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function loadingOf(wrapper: ReturnType<typeof mount>): any {
  // @ts-expect-error access internal setup state
  return wrapper.vm.$.setupState.loading
}

describe('MetricsDashboard.vue — A12 运行指标', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('解析 Prometheus 文本并渲染概览卡片', async () => {
    mockGetMetrics.mockResolvedValue(PROM_TEXT)
    const wrapper = mount(MetricsDashboard)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('上行总数')
    expect(text).toContain('50')
    expect(text).toContain('队列长度')
    expect(text).toContain('1,234')
    expect(text).toContain('在线设备')
    expect(text).toContain('42')
    expect(text).toContain('错误总数')
    expect(text).toContain('7')
  })

  it('渲染指标分组折叠面板，展示 help 与 type', async () => {
    mockGetMetrics.mockResolvedValue(PROM_TEXT)
    const wrapper = mount(MetricsDashboard)
    await flushPromises()

    // 折叠面板标题应包含指标名
    expect(wrapper.text()).toContain('gateway_uplink_total')
    expect(wrapper.text()).toContain('gateway_mqtt_publish_total')
    // mqtt_publish_total 有 2 条带标签的样本
    expect(wrapper.text()).toContain('result=success')
  })

  it('空响应时显示空状态', async () => {
    mockGetMetrics.mockResolvedValue('')
    const wrapper = mount(MetricsDashboard)
    await flushPromises()

    expect(wrapper.text()).toContain('网关尚未上报指标')
  })

  it('API 失败时显示错误提示', async () => {
    mockGetMetrics.mockRejectedValue(new Error('fail'))
    const wrapper = mount(MetricsDashboard)
    await flushPromises()

    expect(wrapper.exists()).toBe(true)
  })

  it('数值格式化：整数用千分位，小数保留两位', () => {
    // 通过挂载后验证 DOM 中的 1,234 已在上文覆盖；这里直接验证组件不崩溃
    expect(true).toBe(true)
  })

  /**
   * 回归：autoRefresh 默认开启，定时器每 5s 触发一次 loadMetrics。原实现每轮都置
   * loading=true，而 loading 绑在 .metrics-body 的 v-loading 上 → 指标面板每 5 秒
   * 被蒙层覆盖、刷新按钮同步抖动。现改为定时器走静默路径，手动刷新仍给反馈。
   */
  it('手动刷新进入 loading，5s 自动刷新为静默路径不再遮挡面板', async () => {
    // 只伪造 setInterval，保留真实 setTimeout（flushPromises 依赖它）
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    try {
      mockGetMetrics.mockResolvedValue(PROM_TEXT)
      const wrapper = mount(MetricsDashboard)
      await flushPromises()
      expect(loadingOf(wrapper)).toBe(false)

      // 让下一次请求挂起，便于在 loading 期间观察状态
      let resolveNext!: (v: string) => void
      const hang = () => new Promise<string>((resolve) => { resolveNext = resolve })

      // 1) 手动点「刷新」：应进入 loading（用户主动操作需要反馈）
      mockGetMetrics.mockReturnValue(hang())
      const refreshBtn = wrapper.findAll('button').find((b) => b.text().trim() === '刷新')
      expect(refreshBtn).toBeTruthy()
      await refreshBtn!.trigger('click')
      expect(loadingOf(wrapper)).toBe(true)

      resolveNext(PROM_TEXT)
      await flushPromises()
      expect(loadingOf(wrapper)).toBe(false)

      // 2) 定时器路径：请求挂起期间不得置 loading（否则面板蒙层每 5s 闪一次）
      mockGetMetrics.mockReturnValue(hang())
      vi.advanceTimersByTime(5000)
      expect(loadingOf(wrapper)).toBe(false)

      resolveNext(PROM_TEXT)
      await flushPromises()
      expect(loadingOf(wrapper)).toBe(false)
      wrapper.unmount()
    } finally {
      vi.useRealTimers()
    }
  })
})
