// ApiRateLimit.vue 单元测试：限流规则列表/新建/删除与访问日志区。
// mock 边界同仓内其他页面测试：@/api/apiRateLimit 全量 mock、confirmDelete 桩、
// ElMessage 替换为 vi.fn（useErrorDisplay 的 showToast 最终也走它）。
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import type { ApiLimitRule, ApiLog } from '@/api/apiRateLimit'
import { formatDateTime } from '@/utils/datetime'

const {
  listRulesMock,
  createRuleMock,
  updateRuleMock,
  deleteRuleMock,
  getRecentLogsMock,
  getStatsMock,
  confirmDeleteMock,
} = vi.hoisted(() => ({
  listRulesMock: vi.fn(),
  createRuleMock: vi.fn(),
  updateRuleMock: vi.fn(),
  deleteRuleMock: vi.fn(),
  getRecentLogsMock: vi.fn(),
  getStatsMock: vi.fn(),
  confirmDeleteMock: vi.fn(),
}))

vi.mock('@/api/apiRateLimit', () => ({
  listRules: listRulesMock,
  createRule: createRuleMock,
  updateRule: updateRuleMock,
  deleteRule: deleteRuleMock,
  getRecentLogs: getRecentLogsMock,
  getStats: getStatsMock,
}))

vi.mock('@/utils/confirm', () => ({
  confirmDelete: confirmDeleteMock,
  confirmAction: vi.fn(),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

import ApiRateLimit from './ApiRateLimit.vue'
import { ElMessage } from 'element-plus'

const RULE: ApiLimitRule = {
  id: 'r1',
  name: '设备列表限流',
  path: '/api/devices',
  method: 'GET',
  limitPerMinute: 60,
  limitPerHour: 1000,
  burst: 20,
  enabled: true,
}

const LOG: ApiLog = {
  id: 1,
  time: 1780000000000,
  path: '/api/devices',
  method: 'GET',
  status: 429,
  latencyMs: 12,
  clientIp: '192.168.1.10',
  limited: true,
}

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
  const wrapper = mount(ApiRateLimit)
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

async function findButton(wrapper: Awaited<ReturnType<typeof mountPage>>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().includes(text))
  expect(btn, `未找到「${text}」按钮`).toBeTruthy()
  return btn!
}

describe('ApiRateLimit.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listRulesMock.mockResolvedValue([RULE])
    getStatsMock.mockResolvedValue({ totalRequests: 128, limitedRequests: 3, avgLatencyMs: 12 })
    getRecentLogsMock.mockResolvedValue([LOG])
    confirmDeleteMock.mockResolvedValue(true)
    deleteRuleMock.mockResolvedValue(undefined)
  })

  it('挂载后加载规则/统计/日志并渲染行', async () => {
    const wrapper = await mountPage()

    expect(listRulesMock).toHaveBeenCalledTimes(1)
    expect(getStatsMock).toHaveBeenCalledTimes(1)
    expect(getRecentLogsMock).toHaveBeenCalledTimes(1)

    const text = wrapper.text()
    // 规则行
    expect(text).toContain('设备列表限流')
    expect(text).toContain('/api/devices')
    // 统计卡片
    expect(text).toContain('128')
    expect(text).toContain('12 ms')
    // 访问日志行：时间走 formatDateTime，限流标记渲染为「限流」
    expect(text).toContain(formatDateTime(LOG.time))
    expect(text).toContain('192.168.1.10')
    expect(text).toContain('限流')
  })

  it('新建规则提交 camelCase 完整载荷并刷新列表', async () => {
    createRuleMock.mockResolvedValue({ ...RULE })
    const wrapper = await mountPage()

    await (await findButton(wrapper, '新建规则')).trigger('click')
    await settle()

    const nameInput = wrapper.find('input[placeholder="如：设备列表限流"]')
    expect(nameInput.exists()).toBe(true)
    await nameInput.setValue('设备列表限流')

    await (await findButton(wrapper, '创建')).trigger('click')
    await settle()

    expect(createRuleMock).toHaveBeenCalledTimes(1)
    expect(createRuleMock).toHaveBeenCalledWith({
      name: '设备列表限流',
      path: '/api/',
      method: '*',
      limitPerMinute: 60,
      limitPerHour: 1000,
      burst: 20,
      enabled: true,
    })
    expect(ElMessage.success).toHaveBeenCalledWith('已创建')
    // 创建成功后重新拉取规则列表
    expect(listRulesMock).toHaveBeenCalledTimes(2)
  })

  it('名称为空时不调用创建接口并提示警告', async () => {
    const wrapper = await mountPage()

    await (await findButton(wrapper, '新建规则')).trigger('click')
    await settle()
    await (await findButton(wrapper, '创建')).trigger('click')
    await settle()

    expect(createRuleMock).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalledWith('请输入规则名称')
  })

  it('确认删除后调用 deleteRule 并刷新列表', async () => {
    const wrapper = await mountPage()

    await (await findButton(wrapper, '删除')).trigger('click')
    await settle()

    expect(confirmDeleteMock).toHaveBeenCalledWith('设备列表限流')
    expect(deleteRuleMock).toHaveBeenCalledWith('r1')
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
    expect(listRulesMock).toHaveBeenCalledTimes(2)
  })

  it('取消删除时不调用删除接口', async () => {
    confirmDeleteMock.mockResolvedValue(false)
    const wrapper = await mountPage()

    await (await findButton(wrapper, '删除')).trigger('click')
    await settle()

    expect(confirmDeleteMock).toHaveBeenCalled()
    expect(deleteRuleMock).not.toHaveBeenCalled()
  })

  it('规则列表接口报错时提示错误且不崩溃', async () => {
    listRulesMock.mockRejectedValue(new Error('boom'))
    const wrapper = await mountPage()

    expect(wrapper.exists()).toBe(true)
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('boom'))
    expect(ElMessage.success).not.toHaveBeenCalled()
  })
})
