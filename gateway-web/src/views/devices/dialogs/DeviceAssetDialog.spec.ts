import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import DeviceAssetDialog from './DeviceAssetDialog.vue'

// vi.mock 工厂会被提升到文件顶部，直接引用顶层 const 会在初始化前触发 TDZ。
// vi.hoisted 把 mock 引用提升到 mock 注册之前。
const {
  mockGetAssetView,
  mockUpsertAssetView,
  mockError,
  mockSuccess,
} = vi.hoisted(() => ({
  mockGetAssetView: vi.fn(),
  mockUpsertAssetView: vi.fn(),
  mockError: vi.fn(),
  mockSuccess: vi.fn(),
}))

vi.mock('@/api/deviceOps', () => ({
  getAssetView: (...args: unknown[]) => mockGetAssetView(...args),
  upsertAssetView: (...args: unknown[]) => mockUpsertAssetView(...args),
  ALL_TAGS: ['重要', '外协'],
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: mockSuccess, error: mockError } }
})

/** 读取指定 placeholder 的输入框当前值 */
function inputValue(wrapper: ReturnType<typeof mount>, placeholder: string): string {
  const input = wrapper.find(`input[placeholder="${placeholder}"]`).element as HTMLInputElement
  return input.value
}

async function settle() {
  await flushPromises()
  await nextTick()
}

function makeAsset(overrides: Record<string, unknown> = {}) {
  return {
    deviceId: 'd1',
    assetNo: 'SN-001',
    vendor: '某厂',
    model: 'M1',
    location: '机房A',
    owner: '张三',
    purchasedAt: '2026-01-01',
    calibratedAt: '2026-06-01',
    tags: ['重要'],
    ...overrides,
  }
}

describe('DeviceAssetDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockGetAssetView.mockResolvedValue(makeAsset())
    mockUpsertAssetView.mockResolvedValue(undefined)
  })

  it('打开时按 deviceId 拉取资产视图并回填', async () => {
    const wrapper = mount(DeviceAssetDialog, {
      props: { modelValue: true, deviceId: 'd1', canWrite: true },
    })
    await settle()
    expect(mockGetAssetView).toHaveBeenCalledWith('d1')
    // 已登记的资产号应回填到表单
    expect(inputValue(wrapper, '请输入资产编号')).toBe('SN-001')
  })

  it('接口失败时按新增处理（空表单而不是崩溃）', async () => {
    mockGetAssetView.mockRejectedValue(new Error('404'))
    const wrapper = mount(DeviceAssetDialog, {
      props: { modelValue: true, deviceId: 'd2', canWrite: true },
    })
    await settle()
    expect(mockGetAssetView).toHaveBeenCalledWith('d2')
    expect(inputValue(wrapper, '请输入资产编号')).toBe('')
    expect(inputValue(wrapper, '请输入厂家')).toBe('')
  })

  it('保存时把表单值转成后端字段并提示成功', async () => {
    const wrapper = mount(DeviceAssetDialog, {
      props: { modelValue: true, deviceId: 'd1', canWrite: true },
    })
    await settle()

    await wrapper.find('input[placeholder="请输入资产编号"]').setValue('SN-002')
    await settle()

    const saveBtn = wrapper.findAll('button').find((b) => b.text().includes('保存'))
    expect(saveBtn).toBeTruthy()
    await saveBtn!.trigger('click')
    await settle()

    expect(mockUpsertAssetView).toHaveBeenCalledWith({
      device_id: 'd1',
      asset_no: 'SN-002',
      vendor: '某厂',
      model: 'M1',
      location: '机房A',
      owner: '张三',
      purchased_at: '2026-01-01',
      calibrated_at: '2026-06-01',
      tags: ['重要'],
    })
    expect(mockSuccess).toHaveBeenCalledWith('资产信息已保存')
    // 保存成功后关闭弹窗
    expect(wrapper.emitted('update:modelValue')?.at(-1)).toEqual([false])
  })

  it('保存失败时提示错误且不关闭弹窗', async () => {
    mockUpsertAssetView.mockRejectedValue(new Error('网络异常'))
    const wrapper = mount(DeviceAssetDialog, {
      props: { modelValue: true, deviceId: 'd1', canWrite: true },
    })
    await settle()

    const saveBtn = wrapper.findAll('button').find((b) => b.text().includes('保存'))
    await saveBtn!.trigger('click')
    await settle()

    expect(mockError).toHaveBeenCalled()
    expect(wrapper.emitted('update:modelValue')).toBeUndefined()
  })

  it('无写权限时保存按钮禁用', async () => {
    const wrapper = mount(DeviceAssetDialog, {
      props: { modelValue: true, deviceId: 'd1', canWrite: false },
    })
    await settle()
    const saveBtn = wrapper.findAll('button').find((b) => b.text().includes('保存'))
    expect(saveBtn?.attributes('disabled')).toBeDefined()
  })

  it('弹窗关闭时不拉取资产（避免无谓请求）', async () => {
    mount(DeviceAssetDialog, {
      props: { modelValue: false, deviceId: 'd1', canWrite: true },
    })
    await settle()
    expect(mockGetAssetView).not.toHaveBeenCalled()
  })
})
