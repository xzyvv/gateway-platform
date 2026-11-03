import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'
import Login from './Login.vue'

const mockLogin = vi.fn()
const mockPush = vi.fn()
const mockGetStatus = vi.fn()
const mockScopedSet = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: mockPush, replace: vi.fn(), back: vi.fn() }),
  useRoute: () => ({ path: '/login', query: {} }),
}))

// 浏览器直连模式：登录页由网关自身托管，不显示「网关地址」与局域网发现
vi.mock('@/api', () => ({
  isTauri: () => false,
  getGatewayBaseUrl: () => 'http://127.0.0.1:8080',
  loadGatewayConfig: vi.fn(async () => {}),
  saveGatewayConfig: vi.fn(async () => {}),
}))

vi.mock('@/api/status', () => ({ getStatus: (...args: unknown[]) => mockGetStatus(...args) }))

vi.mock('@/stores/auth', () => ({
  useAuthStore: () => ({ login: mockLogin, logout: vi.fn() }),
}))

vi.mock('@/composables/system/useGatewayDiscovery', () => ({
  useGatewayDiscovery: () => ({
    found: ref([]),
    scanning: ref(false),
    scan: vi.fn(async () => []),
    cancel: vi.fn(),
  }),
}))

vi.mock('@/utils/gatewayStorage', () => ({
  scopedSet: (...args: unknown[]) => mockScopedSet(...args),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

async function mountLogin() {
  const wrapper = mount(Login, { global: { stubs: { BrandMark: true } } })
  await flushPromises()
  return wrapper
}

function usernameInput(wrapper: ReturnType<typeof mount>) {
  return wrapper.find('input[aria-label="用户名"]')
}

function passwordInput(wrapper: ReturnType<typeof mount>) {
  return wrapper.find('input[aria-label="密码"]')
}

describe('Login.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockGetStatus.mockResolvedValue({})
  })

  it('浏览器模式只要求用户名密码，不显示网关地址输入框', async () => {
    const wrapper = await mountLogin()
    expect(wrapper.text()).toContain('工业网关')
    expect(usernameInput(wrapper).exists()).toBe(true)
    expect(passwordInput(wrapper).exists()).toBe(true)
    expect(wrapper.find('input[aria-label="网关地址"]').exists()).toBe(false)
  })

  it('挂载时真实探测 /health/ready，可达则显示「网关已连接」', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true, status: 200 })
    vi.stubGlobal('fetch', fetchMock)
    const wrapper = await mountLogin()
    expect(fetchMock).toHaveBeenCalledWith('/health/ready', expect.objectContaining({ cache: 'no-store' }))
    expect(wrapper.text()).toContain('网关已连接')
    vi.unstubAllGlobals()
  })

  it('探测失败（网络不可达）时显示「网关无响应」而不是静态「已直连」文案', async () => {
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('offline')))
    const wrapper = await mountLogin()
    expect(wrapper.text()).toContain('网关无响应')
    vi.unstubAllGlobals()
  })

  it('用户名或密码为空时不发起登录请求并给出提示', async () => {
    const wrapper = await mountLogin()
    await wrapper.find('.login-btn').trigger('click')
    await flushPromises()
    expect(mockLogin).not.toHaveBeenCalled()
    const { ElMessage } = await import('element-plus')
    expect(ElMessage.warning).toHaveBeenCalledWith('请输入用户名和密码')
  })

  it('登录成功后跳转总览页，并回填用户名到本地存储', async () => {
    mockLogin.mockResolvedValue(undefined)
    const wrapper = await mountLogin()
    await usernameInput(wrapper).setValue('  admin  ')
    await passwordInput(wrapper).setValue('secret')
    await wrapper.find('.login-btn').trigger('click')
    await flushPromises()

    expect(mockLogin).toHaveBeenCalledWith('admin', 'secret')
    expect(mockScopedSet).toHaveBeenCalledWith('gw_username', 'admin')
    expect(mockPush).toHaveBeenCalledWith('/overview')
  })

  it('状态预热失败不阻断登录成功流程', async () => {
    mockLogin.mockResolvedValue(undefined)
    mockGetStatus.mockRejectedValue(new Error('status unavailable'))
    const wrapper = await mountLogin()
    await usernameInput(wrapper).setValue('admin')
    await passwordInput(wrapper).setValue('secret')
    await wrapper.find('.login-btn').trigger('click')
    await flushPromises()

    expect(mockPush).toHaveBeenCalledWith('/overview')
  })

  it('登录失败时把真实错误原因展示给现场（而非笼统「用户名或密码错误」）', async () => {
    mockLogin.mockRejectedValue(new Error('Network Error'))
    const wrapper = await mountLogin()
    await usernameInput(wrapper).setValue('admin')
    await passwordInput(wrapper).setValue('wrong')
    await wrapper.find('.login-btn').trigger('click')
    await flushPromises()

    const { ElMessage } = await import('element-plus')
    expect(ElMessage.error).toHaveBeenCalledWith('登录失败：Network Error')
    expect(mockPush).not.toHaveBeenCalled()
  })
})
