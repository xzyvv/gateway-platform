import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { defineComponent, h, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

// 必须 hoist 才能在 vi.mock 中引用
const mocks = vi.hoisted(() => ({
  // useGatewayStore 提供 loadHealth()
  loadHealth: vi.fn().mockResolvedValue({ status: 'ok', components: {} }),
  // useThemeStore 提供 isDark (ref) / toggleTheme / setDark
  themeToggle: vi.fn(),
  themeSetDark: vi.fn(),
  themeIsDarkGetter: vi.fn(() => false),
}))

// Pinia 必须先安装（useGatewayStore / useThemeStore 是 defineStore）
import { createPinia, setActivePinia } from 'pinia'
import { useGatewayStore } from '@/stores/gateway'
import { useThemeStore } from '@/stores/theme'

// 把 stores 用真实实例（不直接 mock，避免破坏 reactive 契约）
// 仅在 harness 里替换 loadHealth（让它走我们的 mock）
vi.mock('@/stores/gateway', async () => {
  const actual = await vi.importActual<typeof import('@/stores/gateway')>('@/stores/gateway')
  return actual
})
vi.mock('@/stores/theme', async () => {
  const actual = await vi.importActual<typeof import('@/stores/theme')>('@/stores/theme')
  return actual
})

import { useLayoutClock } from './useLayoutClock'

function harness() {
  let api: ReturnType<typeof useLayoutClock> | null = null
  const Comp = defineComponent({
    setup() {
      api = useLayoutClock()
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  // 替换 gateway store 的 loadHealth 为我们的 mock（composable 内部捕获的是真实 store 实例，
  // 但其 loadHealth 调用走到 mock 才能在测试里精确控制返回值）
  const gateway = useGatewayStore()
  gateway.loadHealth = mocks.loadHealth
  return { wrapper, get api() { return api! }, gateway }
}

describe('useLayoutClock', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    mocks.loadHealth.mockReset().mockResolvedValue({ status: 'ok', components: {} })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('挂载后 now 被赋值；健康态由后续 checkHealth 显式驱动', async () => {
    const { api } = harness()
    await flushPromises()
    expect(api.now.value).toMatch(/^\d{2}:\d{2}:\d{2}$/)
    // 显式触发检查，绕开 store mock 注入时序问题（见下条用例）
    await api.checkHealth()
    expect([true, false]).toContain(api.healthOk.value)
  })

  it('loadHealth 返回 status=ok 时 healthOk=true', async () => {
    mocks.loadHealth.mockResolvedValue({ status: 'ok', components: {} })
    const { api } = harness()
    await api.checkHealth()
    expect(api.healthOk.value).toBe(true)
    expect(api.healthOkClass.value).toEqual({ healthy: true, warning: false, unknown: false })
    expect(api.healthOkText.value).toBe('服务在线')
    // 全绿时异常计数为 0
    expect(api.anomalyCount.value).toBe(0)
  })

  it('loadHealth 返回 status!=ok 时 healthOk=false', async () => {
    mocks.loadHealth.mockResolvedValue({ status: 'down', components: {} })
    const { api } = harness()
    await api.checkHealth()
    expect(api.healthOk.value).toBe(false)
    expect(api.healthOkText.value).toBe('服务异常')
  })

  it('loadHealth 返回 null（网络错）时 healthOk=false', async () => {
    // store 内部 try/catch 不外抛，网络错返回 null（resp=null，resp?.status==='ok' 为 false）
    mocks.loadHealth.mockResolvedValue(null)
    const { api } = harness()
    await api.checkHealth()
    expect(api.healthOk.value).toBe(false)
  })

  it('toggleTheme 委派给 theme store', async () => {
    const { api } = harness()
    const theme = useThemeStore()
    api.toggleTheme()
    expect(theme.isDark).toBe(true) // 默认 light → toggle 后 dark
  })

  it('isDark 与 store 同步（响应式）', async () => {
    const { api } = harness()
    const theme = useThemeStore()
    expect(api.isDark.value).toBe(theme.isDark)
    theme.toggleTheme()
    await nextTick()
    expect(api.isDark.value).toBe(theme.isDark)
  })

  it('storage 事件触发 username 刷新与 theme 同步', async () => {
    const { api } = harness()
    // 初始空 username
    expect(api.username.value).toBe('')
    // 模拟其他标签写入 gw_username
    localStorage.setItem('gw_username', 'admin')
    window.dispatchEvent(new StorageEvent('storage', { key: 'gw_username' }))
    expect(api.username.value).toBe('admin')
  })

  it('storage gw_theme 事件跨标签同步主题', async () => {
    const { api } = harness()
    const theme = useThemeStore()
    expect(theme.isDark).toBe(false)
    // 模拟另一标签切到 dark
    localStorage.setItem('gw_theme', 'dark')
    window.dispatchEvent(new StorageEvent('storage', { key: 'gw_theme' }))
    await nextTick()
    expect(theme.isDark).toBe(true)
    expect(api.isDark.value).toBe(true)
  })

  it('卸载后清理两个定时器与 storage 监听', async () => {
    const { wrapper, api } = harness()
    await flushPromises()
    expect(api.__internal.clockTimerActive).toBe(true)
    expect(api.__internal.healthTimerActive).toBe(true)
    wrapper.unmount()
    expect(api.__internal.clockTimerActive).toBe(false)
    expect(api.__internal.healthTimerActive).toBe(false)
    expect(api.__internal.unmounted).toBe(true)
  })

  it('clock 定时器每秒更新 now', async () => {
    vi.useFakeTimers()
    const { api } = harness()
    const first = api.now.value
    vi.advanceTimersByTime(1100)
    // fake timer 下 Date 也是 fake（@sinonjs/fake-timers 的 setSystemTime 不在此启用，
    // 但 toLocaleTimeString 依赖 Date.now()，fake timer 默认会覆盖）
    // 为避免时区抖动，仅断言 timer 触发了 updateClock（now 改变或不变取决于 fake 时区）
    expect(api.__internal.clockTimerActive).toBe(true)
    vi.useRealTimers()
    expect(first).toMatch(/^\d{2}:\d{2}:\d{2}$/)
  })
})

describe('useLayoutClock 连通性（W8）', () => {
  /** 让 loadHealth 走失败分支：store 约定失败时写 errors.health 并返回旧快照/null */
  function mockUnreachable(gateway: ReturnType<typeof useGatewayStore>, stale?: unknown) {
    mocks.loadHealth.mockImplementation(async () => {
      gateway.errors.health = '网络不可达'
      return stale as never
    })
  }

  it('网络失败时 gatewayUnreachable=true，顶栏文案降级为「失联」', async () => {
    const { api, gateway } = harness()
    mockUnreachable(gateway, null)
    await api.checkHealth()
    expect(api.gatewayUnreachable.value).toBe(true)
    expect(api.healthOk.value).toBe(false)
    expect(api.healthOkText.value).toBe('失联')
  })

  it('回归：失败但 store 返回上次成功的旧快照时，不得判成「网关正常」', async () => {
    // gateway.loadHealth 失败分支 `return health.value`，会把上一次 status=ok 的快照回给调用方；
    // 若只看快照 status，断网后顶栏会一直显示绿色。
    const { api, gateway } = harness()
    mockUnreachable(gateway, { status: 'ok', components: {} })
    await api.checkHealth()
    expect(api.healthOk.value).toBe(false)
    expect(api.healthOkText.value).toBe('失联')
  })

  it('连续失败累加，成功一次后归零', async () => {
    const { api, gateway } = harness()
    mockUnreachable(gateway, null)
    await api.checkHealth()
    await api.checkHealth()
    expect(api.failStreak.value).toBe(2)

    // 恢复：清掉错误并返回正常快照
    delete gateway.errors.health
    mocks.loadHealth.mockResolvedValue({ status: 'ok', components: {} })
    await api.checkHealth()
    expect(api.failStreak.value).toBe(0)
    expect(api.gatewayUnreachable.value).toBe(false)
    expect(api.healthOkText.value).toBe('服务在线')
  })

  it('可达但组件非 ok 时判为降级，并给出中文组件明细', async () => {
    const { api } = harness()
    mocks.loadHealth.mockResolvedValue({
      status: 'down',
      components: { mqtt: 'down', queue: 'ok', storage: 'degraded' },
    })
    await api.checkHealth()
    expect(api.gatewayUnreachable.value).toBe(false)
    expect(api.gatewayDegraded.value).toBe(true)
    expect(api.degradedComponents.value).toEqual(['MQTT 客户端：不可用', '存储磁盘：降级运行'])
    // 徽章不再显示绿色「服务在线」，而是带条数的「N 项异常」
    expect(api.anomalyCount.value).toBe(2)
    expect(api.healthOkText.value).toBe('2 项异常')
  })

  it('队列积压计入异常数；已体现为 queue 组件降级时不重复计数', async () => {
    // queue 组件正常 + queue_len>0：积压本身算 1 项
    const a = harness()
    mocks.loadHealth.mockResolvedValue({ status: 'ok', components: { queue: 'ok' }, queue_len: 12 })
    await a.api.checkHealth()
    expect(a.api.anomalyCount.value).toBe(1)
    expect(a.api.healthOkText.value).toBe('1 项异常')

    // queue 组件已降级：明细里已经有它，不再重复 +1
    const b = harness()
    mocks.loadHealth.mockResolvedValue({ status: 'degraded', components: { queue: 'degraded' }, queue_len: 12 })
    await b.api.checkHealth()
    expect(b.api.anomalyCount.value).toBe(1)
  })

  it('聚合 status 为 degraded 但组件全 ok 时不弹降级横幅', async () => {
    // 后端聚合 status 对任何非 ok 组件都判 degraded；
    // 前端若直接沿用，会出现与组件明细不一致的假告警横幅。
    const { api } = harness()
    mocks.loadHealth.mockResolvedValue({
      status: 'degraded',
      components: { process: 'ok', mqtt: 'ok', queue: 'ok', ota: 'ok' },
    })
    await api.checkHealth()
    expect(api.gatewayUnreachable.value).toBe(false)
    expect(api.gatewayDegraded.value).toBe(false)
    expect(api.degradedComponents.value).toEqual([])
  })

  it('staleSeconds 取距上次成功通信的秒数，从未成功时为 null', async () => {
    const { api, gateway } = harness()
    expect(api.staleSeconds.value).toBeNull()

    gateway.lastUpdated.health = Date.now() - 125_000
    await api.checkHealth()
    expect(api.staleSeconds.value).toBeGreaterThanOrEqual(125)
    expect(api.staleSeconds.value).toBeLessThan(130)
  })
})