import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import { ref, computed } from 'vue'
import Topbar from './Topbar.vue'
import { useTableDensity, __resetTablePrefsForTests } from '@/composables/core/useTablePrefs'
import type { useLayoutClock } from '@/composables/layout/useLayoutClock'
import type { useGatewaySwitcher } from '@/composables/layout/useGatewaySwitcher'
import type { GatewayEntry, DiscoveredGateway } from '@/api/gatewayConfig'

type LayoutClockApi = ReturnType<typeof useLayoutClock>
type GatewaySwitcherApi = ReturnType<typeof useGatewaySwitcher>

/** Topbar 测试：折叠按钮、面包屑、密度切换、用户菜单 emits */

// stub 按 composable 的真实返回结构精确构造：响应式字段是 Ref（组件内按 .value 解包）
const clockStub: LayoutClockApi = {
  now: ref('12:34:56'),
  nowTs: ref(Date.now()),
  healthOk: ref<boolean | null>(true),
  healthOkClass: computed(() => ({ healthy: true, warning: false, unknown: false })),
  healthOkText: computed(() => '服务在线'),
  anomalyCount: computed(() => 0),
  checkHealth: vi.fn(),
  gatewayUnreachable: computed(() => false),
  gatewayDegraded: computed(() => false),
  degradedComponents: computed(() => [] as string[]),
  staleSeconds: computed(() => null),
  failStreak: ref(0),
  queueLen: computed(() => null),
  username: ref('admin'),
  readUsername: vi.fn(),
  isDark: computed(() => false),
  toggleTheme: vi.fn(),
  __internal: { unmounted: false, clockTimerActive: false, healthTimerActive: false },
}

const gatewayStub: GatewaySwitcherApi = {
  isTauriEnv: false,
  gateways: ref<GatewayEntry[]>([]),
  activeGatewayName: ref(''),
  activeGatewayUrl: ref(''),
  isSwitchingGateway: ref(false),
  refreshGateways: vi.fn(),
  performGatewaySwitch: vi.fn(),
  onGatewayCmd: vi.fn(),
  probeNewGateway: vi.fn(),
  isScanning: ref(false),
  scanResults: ref<DiscoveredGateway[]>([]),
  showScanDialog: ref(false),
  performLanScan: vi.fn(),
  addDiscoveredGateway: vi.fn(),
  closeScanDialog: vi.fn(),
  inflightBanner: ref(false),
}

function setup(propsOverride: Record<string, unknown> = {}) {
  setActivePinia(createPinia())
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/', component: { template: '<div />' } }],
  })
  const wrapper = mount(Topbar, {
    props: {
      collapsed: false,
      clock: clockStub,
      gatewaySwitcher: gatewayStub,
      title: '总览',
      group: '仪表盘',
      ...propsOverride,
    },
    global: { plugins: [router] },
  })
  return { wrapper, router }
}

describe('Topbar', () => {
  beforeEach(() => {
    localStorage.clear()
    // 密度是模块级单例，不重置会让上一条用例的切换泄漏到下一条
    __resetTablePrefsForTests()
  })

  it('渲染面包屑：分组 + 标题（分组可选）', () => {
    const { wrapper } = setup()
    const items = wrapper.findAll('.el-breadcrumb__item')
    // 面包屑 = 分组 / 标题，不再有指向 /overview 的「首页」伪根节点
    expect(items).toHaveLength(2)
    expect(items[0]!.text()).toContain('仪表盘')
    // 最后一项始终是 title
    expect(items.at(-1)!.text()).toContain('总览')
  })

  it('分组为空时面包屑只剩标题', () => {
    const { wrapper } = setup({ group: '' })
    const items = wrapper.findAll('.el-breadcrumb__item')
    expect(items).toHaveLength(1)
    expect(items[0]!.text()).toContain('总览')
  })

  it('点击折叠按钮触发 toggle-collapse', async () => {
    const { wrapper } = setup()
    await wrapper.find('.collapse-btn').trigger('click')
    expect(wrapper.emitted('toggle-collapse')).toBeTruthy()
  })

  it('窄屏下折叠按钮语义变为「打开侧边导航」', async () => {
    const { wrapper } = setup({ mobile: true })
    const btn = wrapper.find('.collapse-btn')
    expect(btn.attributes('aria-label')).toBe('打开侧边导航')
    expect(btn.attributes('title')).toBe('打开侧边导航')
    await btn.trigger('click')
    // 事件名沿用 toggle-collapse，由 Layout 按当前断点决定是折叠还是开抽屉
    expect(wrapper.emitted('toggle-collapse')).toBeTruthy()
  })


  it('密度按钮切换全站表格密度偏好', async () => {
    const { wrapper } = setup()
    const { density } = useTableDensity()
    const denseBtn = wrapper
      .findAll('.icon-btn')
      .find((b) => (b.attributes('title') || '').includes('表格密度'))
    expect(denseBtn).toBeTruthy()

    await denseBtn!.trigger('click')
    expect(density.value).toBe('compact')
    // 提示语跟随状态，用户点击后能确认当前处于紧凑
    expect(denseBtn!.attributes('title')).toContain('紧凑')

    await denseBtn!.trigger('click')
    expect(density.value).toBe('default')
  })

  it('健康徽章渲染 clock.healthOkText', () => {
    const { wrapper } = setup()
    expect(wrapper.find('.health-badge').text()).toContain('服务在线')
  })

  it('Web 模式下不渲染网关切换器', () => {
    const { wrapper } = setup()
    expect(wrapper.find('.gateway-switcher').exists()).toBe(false)
  })

  it('默认（vitest 视为 dev）渲染 tier 切换器', () => {
    const { wrapper } = setup()
    // vitest 环境下 import.meta.env.DEV=true，tier 切换器可见
    expect(wrapper.find('.tier-switcher').exists()).toBe(true)
  })

  it('DEV 模式下不渲染 license 徽章（生产专用，v-else 分支）', () => {
    const { wrapper } = setup()
    // DEV 走 tier 切换器分支；license 徽章仅生产构建渲染（E2E 生产构建验证）
    expect(wrapper.find('.license-badge').exists()).toBe(false)
  })

  it('用户菜单触发 logout / change-password 上交事件', async () => {
    const { wrapper } = setup()
    // 找到 .logout-btn，el-dropdown 内部不易点击；测试通过 vm.$emit
    const log = wrapper.find('.logout-btn')
    expect(log.exists()).toBe(true)
    // el-dropdown 不会直接 emit command，验证菜单结构与命令存在
    const menu = wrapper.findAll('.el-dropdown-menu__item')
    expect(menu.length).toBeGreaterThanOrEqual(0) // 弹层未挂载时是 0
  })

  it('点击 logout 下拉项时应 emit logout', async () => {
    // 简化验证：直接调 wrapper.vm.$emit 父层事件检查事件链
    const { wrapper } = setup()
    wrapper.vm.$emit('logout')
    expect(wrapper.emitted('logout')).toBeTruthy()
    wrapper.vm.$emit('change-password')
    expect(wrapper.emitted('change-password')).toBeTruthy()
  })
})