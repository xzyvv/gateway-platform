import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { ref, reactive } from 'vue'
import UplinkTester from './UplinkTester.vue'

const mockTestSend = vi.fn()
const mockLoadFromConfig = vi.fn()
const mockApplyTemplate = vi.fn()
const mockRandomizeData = vi.fn()
const mockCopyRendered = vi.fn()
const mockLoadHistory = vi.fn()
const mockConfirmClearHistory = vi.fn()
const mockReuseHistory = vi.fn()
const mockGoConfig = vi.fn()

vi.mock('@/composables/uplink/useUplinkTester', () => ({
  useUplinkTester: () => ({
    testing: ref(false),
    sending: ref(false),
    lastResult: ref(null),
    history: ref([]),
    historyLoading: ref(false),
    state: reactive({
      template: '{"device":"${device_id}","value":${value}}',
      vars: {
        device_id: 'dianbiao1',
        device_group: 'chejianA',
        point_id: 'voltage_a',
        unit: 'V',
        quality_text: 'good',
      },
    }),
    valueNum: ref(220.5),
    staleNum: ref(0),
    rendered: ref('{"device":"dianbiao1","value":220.5}'),
    templateBytes: ref(40),
    renderedBytes: ref(34),
    compileError: ref(null),
    BUILTIN_TEMPLATES: [
      { id: 'json', name: 'JSON', template: '{"device":"${device_id}","value":${value}}' },
      { id: 'csv', name: 'CSV', template: '${device_id},${value}' },
    ],
    applyTemplate: mockApplyTemplate,
    randomizeData: mockRandomizeData,
    loadFromConfig: mockLoadFromConfig,
    goConfig: mockGoConfig,
    copyRendered: mockCopyRendered,
    loadHistory: mockLoadHistory,
    confirmClearHistory: mockConfirmClearHistory,
    reuseHistory: mockReuseHistory,
    testSend: mockTestSend,
    formatTime: (_t: number) => '00:00:00',
  }),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

describe('UplinkTester.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders title and template/preview sections', async () => {
    const wrapper = mount(UplinkTester)
    await flushPromises()
    const text = wrapper.text()
    expect(text).toContain('上行测试')
    expect(text).toContain('模板内容')
    expect(text).toContain('测试数据 / 渲染预览')
  })

  it('shows compile status tag (ready) and byte counts', async () => {
    const wrapper = mount(UplinkTester)
    await flushPromises()
    const text = wrapper.text()
    expect(text).toContain('就绪')
    expect(text).toContain('字节数')
    expect(text).toContain('渲染结果')
  })

  it('renders builtin preset buttons and applyTemplate is called on click', async () => {
    const wrapper = mount(UplinkTester)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('JSON'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(mockApplyTemplate).toHaveBeenCalledWith('json')
  })

  it('clicking load-from-config button calls loadFromConfig', async () => {
    const wrapper = mount(UplinkTester)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('从已保存配置加载'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(mockLoadFromConfig).toHaveBeenCalled()
  })

  it('clicking real-uplink-test button calls testSend(true)', async () => {
    const wrapper = mount(UplinkTester)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('真实上行测试'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(mockTestSend).toHaveBeenCalledWith(true)
  })

  it('shows empty history state when history is empty', async () => {
    const wrapper = mount(UplinkTester)
    await flushPromises()
    expect(wrapper.text()).toContain('暂无测试历史')
  })
})