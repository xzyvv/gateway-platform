import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ChangePassword from './ChangePassword.vue'

const mockChangePassword = vi.fn()
const mockLogout = vi.fn()
const mockPush = vi.fn()
const mockBack = vi.fn()

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: mockPush, replace: vi.fn(), back: mockBack }),
  useRoute: () => ({ path: '/change-password', query: {} }),
}))

vi.mock('@/api/auth', () => ({ changePassword: (...args: unknown[]) => mockChangePassword(...args) }))

vi.mock('@/stores/auth', () => ({
  useAuthStore: () => ({ logout: mockLogout, login: vi.fn() }),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

function inputs(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAll('input')
}

async function fill(wrapper: ReturnType<typeof mount>, current: string, next: string, confirm: string) {
  await wrapper.find('input[aria-label="当前密码"]').setValue(current)
  await wrapper.find('input[aria-label="新密码"]').setValue(next)
  await wrapper.find('input[aria-label="确认新密码"]').setValue(confirm)
  await flushPromises()
}

describe('ChangePassword.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockChangePassword.mockResolvedValue(undefined)
  })

  it('渲染当前密码 / 新密码 / 确认新密码三个输入框', async () => {
    const wrapper = mount(ChangePassword)
    await flushPromises()
    expect(wrapper.text()).toContain('修改密码')
    expect(inputs(wrapper)).toHaveLength(3)
    expect(wrapper.find('input[aria-label="当前密码"]').exists()).toBe(true)
    expect(wrapper.find('input[aria-label="新密码"]').exists()).toBe(true)
    expect(wrapper.find('input[aria-label="确认新密码"]').exists()).toBe(true)
  })

  it('密码强度按长度与字符多样性分级：12 位混合字符显示「强」', async () => {
    const wrapper = mount(ChangePassword)
    await flushPromises()
    await wrapper.find('input[aria-label="新密码"]').setValue('Abc123!@#xyz')
    await flushPromises()
    expect(wrapper.text()).toContain('强')
  })

  it('两次输入不一致时本地校验拦下，不调用后端', async () => {
    const wrapper = mount(ChangePassword)
    await flushPromises()
    await fill(wrapper, 'oldpass1', 'newpass12', 'newpass13')
    await wrapper.find('.cp-btn').trigger('click')
    await flushPromises()

    expect(mockChangePassword).not.toHaveBeenCalled()
  })

  it('新密码少于 6 位时本地校验拦下，不调用后端', async () => {
    const wrapper = mount(ChangePassword)
    await flushPromises()
    await fill(wrapper, 'oldpass1', 'abc12', 'abc12')
    await wrapper.find('.cp-btn').trigger('click')
    await flushPromises()

    expect(mockChangePassword).not.toHaveBeenCalled()
  })

  it('校验通过后提交：调用改密接口、退出登录并回到登录页', async () => {
    const wrapper = mount(ChangePassword)
    await flushPromises()
    await fill(wrapper, 'oldpass1', 'newpass12', 'newpass12')
    await wrapper.find('.cp-btn').trigger('click')
    await flushPromises()

    expect(mockChangePassword).toHaveBeenCalledWith('oldpass1', 'newpass12')
    expect(mockLogout).toHaveBeenCalled()
    expect(mockPush).toHaveBeenCalledWith('/login')
  })

  it('后端 401（当前密码错误）给出明确提示，且不清除登录态', async () => {
    mockChangePassword.mockRejectedValue({ response: { status: 401 } })
    const wrapper = mount(ChangePassword)
    await flushPromises()
    await fill(wrapper, 'wrongpass', 'newpass12', 'newpass12')
    await wrapper.find('.cp-btn').trigger('click')
    await flushPromises()

    const { ElMessage } = await import('element-plus')
    expect(ElMessage.warning).toHaveBeenCalledWith('当前密码错误')
    expect(mockLogout).not.toHaveBeenCalled()
  })

  it('非 401 失败给出带动作语境的提示', async () => {
    mockChangePassword.mockRejectedValue({ response: { status: 500 } })
    const wrapper = mount(ChangePassword)
    await flushPromises()
    await fill(wrapper, 'oldpass1', 'newpass12', 'newpass12')
    await wrapper.find('.cp-btn').trigger('click')
    await flushPromises()

    const { ElMessage } = await import('element-plus')
    expect(ElMessage.error).toHaveBeenCalledWith('密码修改失败，请稍后重试')
  })
})
