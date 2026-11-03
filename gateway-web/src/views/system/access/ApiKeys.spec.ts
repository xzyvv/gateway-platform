// ApiKeys.vue 单元测试：列表/创建（完整 Key 仅展示一次）/删除。
// 安全模型断言重点：创建成功对话框展示完整 Key，关闭后表格只剩 keyPreview。
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import type { ApiKey, CreatedApiKey } from '@/api/apiKeys'

const {
  listApiKeysMock,
  createApiKeyMock,
  setApiKeyEnabledMock,
  deleteApiKeyMock,
  confirmDeleteMock,
} = vi.hoisted(() => ({
  listApiKeysMock: vi.fn(),
  createApiKeyMock: vi.fn(),
  setApiKeyEnabledMock: vi.fn(),
  deleteApiKeyMock: vi.fn(),
  confirmDeleteMock: vi.fn(),
}))

vi.mock('@/api/apiKeys', () => ({
  listApiKeys: listApiKeysMock,
  createApiKey: createApiKeyMock,
  setApiKeyEnabled: setApiKeyEnabledMock,
  deleteApiKey: deleteApiKeyMock,
  // 与 src/api/apiKeys.ts 的 ALL_SCOPES 保持一致：表单默认取第 0 项
  getAllScopes: () => [
    'devices:read',
    'devices:write',
    'points:read',
    'points:write',
    'config:read',
    'config:write',
    'history:read',
  ],
}))

vi.mock('@/utils/confirm', () => ({
  confirmDelete: confirmDeleteMock,
  confirmAction: vi.fn(),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

import ApiKeys from './ApiKeys.vue'
import { ElMessage } from 'element-plus'

const FULL_KEY = 'gw_full_secret_key_0123456789abcdef'

const KEY: ApiKey = {
  id: 'k1',
  name: 'CI 机器人',
  keyPreview: 'gw_full_…cdef',
  scopes: ['points:read'],
  enabled: true,
  createdAt: 1780000000000,
  callCount: 3,
}

const CREATED: CreatedApiKey = { ...KEY, id: 'k2', name: '新密钥', key: FULL_KEY }

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
})

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(ApiKeys)
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

async function findButton(wrapper: Awaited<ReturnType<typeof mountPage>>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().includes(text))
  expect(btn, `未找到「${text}」按钮`).toBeTruthy()
  return btn!
}

describe('ApiKeys.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listApiKeysMock.mockResolvedValue([KEY])
    deleteApiKeyMock.mockResolvedValue(undefined)
    confirmDeleteMock.mockResolvedValue(true)
  })

  it('挂载后调用 listApiKeys 并渲染名称/preview/调用次数', async () => {
    const wrapper = await mountPage()

    expect(listApiKeysMock).toHaveBeenCalledTimes(1)
    const text = wrapper.text()
    expect(text).toContain('CI 机器人')
    expect(text).toContain('gw_full_…cdef')
    expect(text).toContain('3')
    // 列表行永不出现完整凭证（预览列只有 keyPreview）
    expect(text).not.toContain(FULL_KEY)
  })

  it('创建成功后对话框展示完整 Key，关闭后列表仅剩 preview', async () => {
    createApiKeyMock.mockResolvedValue(CREATED)
    const wrapper = await mountPage()

    await (await findButton(wrapper, '新建 API Key')).trigger('click')
    await settle()

    const nameInput = wrapper.find('input[placeholder="为该 API Key 起个名字"]')
    expect(nameInput.exists()).toBe(true)
    await nameInput.setValue('新密钥')

    await (await findButton(wrapper, '创建')).trigger('click')
    await settle()

    // 请求载荷：名称 + 默认 scopes（allScopes[0]）+ 有效期 0 → undefined
    expect(createApiKeyMock).toHaveBeenCalledWith('新密钥', ['devices:read'], undefined)

    // 创建成功对话框完整展示 Key（仅此一次）
    expect(wrapper.text()).toContain('API Key 已创建')
    expect(wrapper.text()).toContain(FULL_KEY)
    expect(wrapper.text()).toContain('请立即复制并妥善保存')

    // 关闭对话框后：表格只展示 preview，完整 Key 不再出现在表格中
    await (await findButton(wrapper, '我已保存')).trigger('click')
    await settle()

    expect(listApiKeysMock).toHaveBeenCalledTimes(2)
    const tableText = wrapper.find('.data-table-wrap').text()
    expect(tableText).toContain('gw_full_…cdef')
    expect(tableText).not.toContain(FULL_KEY)
  })

  it('名称为空时不调用创建接口并提示警告', async () => {
    const wrapper = await mountPage()

    await (await findButton(wrapper, '新建 API Key')).trigger('click')
    await settle()
    await (await findButton(wrapper, '创建')).trigger('click')
    await settle()

    expect(createApiKeyMock).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalledWith('请输入名称')
  })

  it('确认删除后调用 deleteApiKey 并刷新列表', async () => {
    const wrapper = await mountPage()

    await (await findButton(wrapper, '删除')).trigger('click')
    await settle()

    expect(confirmDeleteMock).toHaveBeenCalledWith('CI 机器人')
    expect(deleteApiKeyMock).toHaveBeenCalledWith('k1')
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
    expect(listApiKeysMock).toHaveBeenCalledTimes(2)
  })

  it('删除接口报错时给出带动作语境的错误提示且不崩溃', async () => {
    deleteApiKeyMock.mockRejectedValue(new Error('500'))
    const wrapper = await mountPage()

    await (await findButton(wrapper, '删除')).trigger('click')
    await settle()

    expect(deleteApiKeyMock).toHaveBeenCalledWith('k1')
    expect(ElMessage.error).toHaveBeenCalledWith('删除 API Key 失败，请稍后重试')
    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(wrapper.exists()).toBe(true)
  })
})
