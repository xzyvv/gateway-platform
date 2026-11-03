// Config.vue 冒烟测试：运行参数是核心写操作页（总线/全局配置的增删改与保存）。
// useConfigActions 整体 mock（含其内部 onMounted 加载逻辑——mock 边界内不自动触发，
// 故只断言骨架与 mock 数据渲染，加载逻辑由 useConfigActions 自己的测试覆盖）。
//
// C4（2026-10-12 补充）：除骨架验证外，反向覆盖以下交互路径——
// - buses 非空时渲染总线行（BaudrateSelect 等子组件实例数）
// - 空态不渲染总线行
// - addBus / removeBus 触发对应 stub
// - JSON tab 切换触发 syncConfigToRaw
// - 端口重复时 busPortErrors 实时反馈
import { describe, it, expect, vi } from 'vitest'
import { ref, reactive, computed } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import Config from './RuntimeConfig.vue'
import { createSeedConfig } from '@/utils/configDefaults'
import type { BusConfig } from '@/api/types'

/** 最小合法总线配置：满足 BusConfig 全部必填字段（测试仅关心 port_name） */
function makeBus(port_name: string): BusConfig {
  return {
    port_name,
    baud_rate: 9600,
    parity: 'none',
    data_bits: 8,
    stop_bits: 1,
    timeout_ms: 1000,
    protocol: 'modbus',
    retry_count: null,
    retry_delay_ms: 100,
    de_pin: null,
    modbus_mode: 'rtu',
    dlt645_version: '2007',
    variant: null,
    asdu_addr_len: null,
    opcua: null,
    tcp_addr: null,
    tcp_concurrent: null,
    channel_capacity: 128,
    reconnect_backoff: { seconds: [1, 2, 5, 10, 30], jitter_ratio: 0 },
  }
}

const baseMock = () => {
  // 与生产 useConfigActions 保持一致：reactive 种子补全全部顶层键。
  // 各 tab 子组件以 v-model 接收对应配置节，缺键会触发 Vue prop 校验告警，
  // 故这里直接用生产同款种子（buses/points 为空数组）。
  const config = reactive(createSeedConfig())
  const busPortErrors = computed(() => {
    const errs = new Map<number, string>()
    const seen = new Map<string, number>()
    config.buses.forEach((b, i) => {
      const key = b.port_name.trim()
      if (!key) return
      const first = seen.get(key)
      if (first !== undefined) errs.set(i, `端口 "${key}" 与第 ${first + 1} 条重复`)
      else seen.set(key, i)
    })
    return errs
  })
  return {
    activeTab: ref('buses'),
    showValidate: ref(false),
    showDiff: ref(false),
    reloading: ref(false),
    // 修复(审查#B7)：与生产 useConfigActions 对齐，补上保存按钮在飞标志
    saving: ref(false),
    config,
    gatewayConfig: computed(() => config),
    originalToken: ref(''),
    rawText: ref('{}'),
    parseError: ref(false),
    parseErrorMessage: ref(''),
    tokenStatusText: computed(() => '未修改' as '当前值' | '保留原值' | '已修改'),
    rawJsonHtml: computed(() => ''),
    // P2b：本地草稿相关状态与动作（与生产 useConfigActions 对齐）
    hasDraft: ref(false),
    draftSavedAt: ref<number | null>(null),
    saveDraft: vi.fn(),
    restoreDraft: vi.fn(),
    discardDraft: vi.fn(),
    busPortErrors,
    loadConfig: vi.fn(),
    saveConfig: vi.fn(),
    doReload: vi.fn(),
    exportConfig: vi.fn(),
    previewDiff: vi.fn(),
    addBus: vi.fn(() => { config.buses.push(makeBus('/dev/ttyUSB0')) }),
    removeBus: vi.fn((idx: number) => { config.buses.splice(idx, 1) }),
    resetRawText: vi.fn(),
    onRawJsonInput: vi.fn(),
    syncConfigToRaw: vi.fn(),
    syncScroll: vi.fn(),
    onLocateIssue: vi.fn(),
  }
}

vi.mock('@/composables/config/useConfigActions', () => ({
  // 必须是 vi.fn()：个别用例需要 .mockImplementation() 换装特定 mock 实例
  useConfigActions: vi.fn(() => baseMock()),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

function mountConfig() {
  return mount(Config, {
    global: {
      stubs: ['ConfigChangePreview', 'ConfigValidateDialog'],
    },
  })
}

describe('Config.vue 骨架', () => {
  it('挂载渲染配置页骨架（tab 容器）', () => {
    const wrapper = mountConfig()
    expect(wrapper.exists()).toBe(true)
    expect(wrapper.html().length).toBeGreaterThan(100)
    expect(wrapper.findComponent({ name: 'ElTabs' }).exists()).toBe(true)
    wrapper.unmount()
  })
})

describe('Config.vue 总线 tab 渲染（C4）', () => {
  it('空配置状态下不渲染总线行且不报错', () => {
    const wrapper = mountConfig()
    expect(wrapper.findComponent({ name: 'BaudrateSelect' }).exists()).toBe(false)
    wrapper.unmount()
  })

  it('config.buses 非空时按数组长度渲染 BaudrateSelect 行', async () => {
    const mock = baseMock()
    vi.mocked(await import('@/composables/config/useConfigActions')).useConfigActions.mockImplementation(() => mock)
    mock.config.buses.push(
      makeBus('/dev/ttyS1'),
      makeBus('/dev/ttyS2'),
    )
    const wrapper = mountConfig()
    await flushPromises()
    expect(wrapper.findAllComponents({ name: 'BaudrateSelect' })).toHaveLength(2)
    wrapper.unmount()
  })
})

describe('Config.vue 总线 CRUD 触发（C4）', () => {
  it('点新增总线触发 addBus stub', async () => {
    const mock = baseMock()
    vi.mocked(await import('@/composables/config/useConfigActions')).useConfigActions.mockImplementation(() => mock)
    const wrapper = mountConfig()
    const addBtn = wrapper.findAll('button').find((b) => b.text().includes('新增总线'))
    expect(addBtn).toBeTruthy()
    await addBtn!.trigger('click')
    expect(mock.addBus).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })
})
