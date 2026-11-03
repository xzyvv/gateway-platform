import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick, ref, reactive } from 'vue'
import Devices from './DeviceList.vue'

const mockLoadDevices = vi.fn()
const mockAddDevice = vi.fn()
const mockBatchCheck = vi.fn()
const mockEditDevice = vi.fn()
const mockRemoveDevice = vi.fn()

// 设备搜索走 useQueryState（URL 记忆），需要完整的 route/currentRoute 形状
vi.mock('vue-router', () => ({
  useRouter: () => ({
    push: vi.fn(),
    replace: vi.fn(),
    currentRoute: { value: { query: {}, path: '/devices' } },
  }),
  useRoute: () => ({ query: {}, path: '/devices' }),
}))

vi.mock('@/composables/devices/useDevices', () => ({
  useDevices: () => ({
    devices: ref([
      { id: 'd1', name: 'dianbiao1', template: 'tpl1', bus: 'bus1', protocol: 'modbus', address: '1', enabled: true, group: '', tags: [] },
      { id: 'd2', name: 'shuibiao2', template: 'tpl2', bus: 'bus2', protocol: 'dlt645', address: '2', enabled: false, group: 'jifangA', tags: ['tag1'] },
    ]),
    loading: ref(false),
    // 列表加载失败原因；默认空串（正常态）
    loadError: ref(''),
    saving: ref(false),
    testingConn: ref(false),
    showCreate: ref(false),
    showDiscover: ref(false),
    editing: ref(false),
    checking: ref(false),
    form: reactive({ id: '', name: '', template: '', bus: '', protocol: 'modbus', address: '', group: '', tags: [] }),
    rules: {},
    // 弹窗脏守卫：本 spec 不验守卫行为，给空实现即可
    formGuard: { beforeClose: (done: () => void) => done(), close: vi.fn(), confirmClose: vi.fn(async () => true), isDirty: () => false, markClean: vi.fn() },
    commsStats: ref({}),
    commsQualityType: () => 'info' as const,
    commsQualityText: () => 'unknown',
    protocolLabel: { modbus: 'Modbus', dlt645: 'DLT645', cjt188: 'CJT188' },
    protocolType: () => 'info',
    protocolOptions: [
      { value: 'modbus', label: 'Modbus' },
      { value: 'dlt645', label: 'DLT645' },
      { value: 'cjt188', label: 'CJT188' },
    ],
    protocolAddressPlaceholder: () => 'slave-id',
    rowClass: () => '',
    loadDevices: mockLoadDevices,
    batchCheck: mockBatchCheck,
    addDevice: mockAddDevice,
    editDevice: mockEditDevice,
    onDialogOpen: vi.fn(),
    testDeviceConn: vi.fn(),
    saveDevice: vi.fn(),
    removeDevice: mockRemoveDevice,
    onDiscoverAdd: vi.fn(),
    DeviceDiscoverDialog: { name: 'DeviceDiscoverDialog', render: () => null },
  }),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('Devices.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders count badge in toolbar without duplicating the page title', async () => {
    const wrapper = mount(Devices)
    await settle()
    const text = wrapper.text()
    // 页面标题由 HubShell 按页签（设备清单）渲染，卡片头只留数量与操作
    expect(text).not.toContain('设备管理')
    expect(wrapper.find('.list-toolbar').exists()).toBe(true)
    expect(text).toContain('2 台')
  })

  it('renders toolbar buttons', async () => {
    const wrapper = mount(Devices)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('刷新')
    expect(text).toContain('扫描发现')
    expect(text).toContain('一键连通性检测')
    expect(text).toContain('新增设备')
  })

  it('设备状态按三态渲染：无采集证据的设备显示「未知」而非默认在线', async () => {
    const wrapper = mount(Devices)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('dianbiao1')
    expect(text).toContain('shuibiao2')
    // P0-3：mock 未构造 backoff/抓包证据 → 不能靠 enabled 推断在线，
    // 必须如实显示「未知」（在线/离线由真实采集证据决定）
    expect(text).not.toContain('在线')
    expect(text).toContain('未知')
  })

  it('clicking refresh button calls loadDevices', async () => {
    const wrapper = mount(Devices)
    await settle()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('刷新'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(mockLoadDevices).toHaveBeenCalled()
  })

  it('clicking add-device button calls addDevice', async () => {
    const wrapper = mount(Devices)
    await settle()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('新增设备'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(mockAddDevice).toHaveBeenCalled()
  })
})