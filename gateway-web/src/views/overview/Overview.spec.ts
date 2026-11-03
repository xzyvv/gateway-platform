import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick, ref, reactive, computed } from 'vue'
import Dashboard from './Overview.vue'

/** loading 在挂载时读取，便于用例分别模拟「首屏加载中」与「已加载完成」 */
const m = vi.hoisted(() => ({ loading: false, stale: false }))

// Mock child view modules to avoid loading echarts and other heavy deps
vi.mock('@/components/overview/DeviceSummary.vue', () => ({ default: { name: 'DeviceSummary', render: () => null } }))
vi.mock('./HealthStatus.vue', () => ({ default: { name: 'HealthStatus', render: () => null } }))

vi.mock('@/composables/dashboard/useDashboardData', () => ({
  useDashboardData: () => ({
    showLayout: ref(false),
    layoutSections: ref([
      { key: 'devices', label: 'devices', visible: true },
      { key: 'health', label: 'health', visible: true },
    ]),
    layoutToggle: vi.fn(),
    layoutReset: vi.fn(),
    isVisible: (_key: string) => true,
    goDevice: vi.fn(),
    summary: reactive({
      onlineDevices: 5,
      totalDevices: 10,
      totalPoints: 100,
      buses: 3,
      alerts: 2,
      collectToday: 5000,
      gatewayOnline: true,
    }),
    ringBg: () => 'conic-gradient(red 0deg)',
    formatNum: (n: number) => String(n),
    version: reactive({ version: '1.0.0', build_time: '2024-01-01', git_hash: 'abc123' }),
    healthRows: ref([{ name: 'mqtt', status: 'ok' }]),
    // B1：页头健康结论徽章取自聚合报告的整体状态
    overallStatus: computed(() => ({ value: 'ok' as const, label: '正常', type: 'success' as const })),
    deviceList: ref([{ id: 'd1', name: 'dianbiao1' }]),
    loading: ref(m.loading),
    healthSignals: computed(() => [
      {
        id: 'device-offline',
        severity: 'warning' as const,
        title: '3 台设备离线',
        detail: '在线 7/10 台，离线设备的点位不会产生数据。',
        action: '查看设备管理',
        to: { path: '/devices' },
      },
    ]),
    lastUpdate: ref(new Date('2024-01-01T00:00:00Z')),
    lastUpdateText: ref('just-now'),
    dataStale: ref(m.stale),
    dataStaleText: ref('离线旧数据（3 分钟前）'),
    refresh: vi.fn(),
  }),
}))

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('Dashboard.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    m.loading = false
    m.stale = false
  })

  it('首屏加载中渲染骨架屏，而不是覆盖整页的转圈蒙层', async () => {
    m.loading = true
    const wrapper = mount(Dashboard)
    await settle()

    expect(wrapper.find('.dash-skeleton').exists()).toBe(true)
    expect(wrapper.find('.dashboard').attributes('aria-busy')).toBe('true')
    // 骨架期间不渲染真实卡片，避免"空数字闪一下再变"
    expect(wrapper.find('.sum-card').exists()).toBe(false)
    expect(wrapper.find('.el-loading-mask').exists()).toBe(false)
  })

  it('renders summary cards with device/point/bus/alert values', async () => {
    const wrapper = mount(Dashboard)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('在线设备 / 总数')
    expect(text).toContain('点位总数')
    expect(text).toContain('通信总线')
    expect(text).toContain('今日告警')
    expect(text).toContain('今日采集量')
    expect(text).toContain('网关状态')
  })

  it('汇总卡是按钮且点击直达对应页面', async () => {
    const push = vi.fn()
    const wrapper = mount(Dashboard, { global: { mocks: { $router: { push } } } })
    await settle()
    const cards = wrapper.findAll('button.sum-card')
    expect(cards).toHaveLength(6)
    // 卡片本身就是入口：不需要用户先记住"设备数要去设备页看"
    await cards[1]!.trigger('click')
    expect(push).toHaveBeenCalledWith('/points')
    await cards[3]!.trigger('click')
    expect(push).toHaveBeenCalledWith('/system?tab=logs')
  })

  it('renders version info section with version/build/git', async () => {
    const wrapper = mount(Dashboard)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('版本信息')
    expect(text).toContain('1.0.0')
    expect(text).toContain('abc123')
  })

  it('renders layout-settings button', async () => {
    const wrapper = mount(Dashboard)
    await settle()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('布局设置'))
    expect(btn).toBeTruthy()
  })

  it('gateway online shows running status', async () => {
    const wrapper = mount(Dashboard)
    await settle()
    expect(wrapper.text()).toContain('运行中')
  })

  it('has link to MetricsDashboard', async () => {
    const wrapper = mount(Dashboard)
    await settle()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('查看运行指标'))
    expect(btn).toBeTruthy()
  })

  it('renders the attention panel with abnormal signals from the data layer', async () => {
    const wrapper = mount(Dashboard)
    await settle()
    // 异常聚合面板必须真实出现在汇总卡片之后，且拿到的是数据层派生的信号
    const panel = wrapper.findComponent({ name: 'AttentionPanel' })
    expect(panel.exists()).toBe(true)
    expect(panel.props('signals')).toHaveLength(1)
    expect(wrapper.text()).toContain('需要关注')
    expect(wrapper.text()).toContain('3 台设备离线')
  })

  it('SW 缓存回退时页脚标记「离线旧数据」，不与「刚刚更新」混淆', async () => {
    m.stale = true
    const wrapper = mount(Dashboard)
    await settle()
    const hint = wrapper.find('.stale-hint')
    expect(hint.exists()).toBe(true)
    expect(hint.text()).toContain('离线旧数据')
  })
})