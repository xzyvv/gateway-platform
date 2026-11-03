import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElCheckbox, ElCheckboxGroup, ElInput, ElMessage } from 'element-plus'

/**
 * D1：设备管理 › 设备分组行为测试。
 *
 * 分组的写接口是「增量」的，这一页有两个历史坑，必须钉住：
 *   1. 编辑分组时若不把原有 device_ids 一起提交，设备会被静默清空；
 *   2. 管理设备若改成整体覆盖式写入，会抹掉并发期间别人加进来的设备。
 * 另外「名称为空不许保存」「删除必须二次确认」这两条是现场误操作的最后一道闸。
 */

const { showToast } = vi.hoisted(() => ({ showToast: vi.fn() }))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('vue-router', () => ({ useRoute: () => ({ path: '/devices' }) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({
  useErrorDisplay: () => ({ showToast }),
}))
vi.mock('@/composables/core/useDialogDirtyGuard', () => ({
  useDialogDirtyGuard: () => ({ beforeClose: (done: () => void) => done(), close: () => {} }),
}))
vi.mock('@/utils/confirm', () => ({ confirmDelete: vi.fn() }))
vi.mock('@/api/deviceOps', () => ({
  listGroupsView: vi.fn(),
  createGroupView: vi.fn(),
  updateGroupView: vi.fn(),
  deleteGroup: vi.fn(),
  addDeviceToGroup: vi.fn(),
  removeDeviceFromGroup: vi.fn(),
  presetColors: ['#1677ff', '#52c41a', '#faad14'],
}))
vi.mock('@/api/devices', () => ({ listDevices: vi.fn() }))

import DeviceGroups from './DeviceGroups.vue'
import { confirmDelete } from '@/utils/confirm'
import {
  listGroupsView,
  createGroupView,
  updateGroupView,
  deleteGroup,
  addDeviceToGroup,
  removeDeviceFromGroup,
} from '@/api/deviceOps'
import { listDevices } from '@/api/devices'

function groups() {
  return [
    { id: 'g-1', name: 'A 组', description: '一号车间', color: '#1677ff', deviceIds: ['dev-1'] },
    { id: 'g-2', name: 'B 组', description: '', color: '#52c41a', deviceIds: [] },
  ]
}

async function settle() {
  await flushPromises()
  await nextTick()
}

const wrappers: Array<ReturnType<typeof mount>> = []

async function mountPage() {
  const wrapper = mount(DeviceGroups, { attachTo: document.body })
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

/** 可点击的容器：VueWrapper 与 DOMWrapper 都满足 */
type Scope = { findAll(selector: string): Array<{ text(): string; trigger(event: string): Promise<void> }> }

async function clickButton(scope: Scope, text: string) {
  const btn = scope.findAll('button').find((b) => b.text() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  await btn!.trigger('click')
  await settle()
}

describe('DeviceGroups 设备分组', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(listGroupsView).mockResolvedValue(structuredClone(groups()) as never)
    vi.mocked(listDevices).mockResolvedValue([
      { id: 'dev-1', name: '电表一号' },
      { id: 'dev-2', name: '压力表' },
    ] as never)
    vi.mocked(createGroupView).mockResolvedValue(undefined as never)
    vi.mocked(updateGroupView).mockResolvedValue(undefined as never)
    vi.mocked(deleteGroup).mockResolvedValue(undefined as never)
    vi.mocked(addDeviceToGroup).mockResolvedValue(undefined as never)
    vi.mocked(removeDeviceFromGroup).mockResolvedValue(undefined as never)
    vi.mocked(confirmDelete).mockResolvedValue(true as never)
  })

  afterEach(() => {
    for (const w of wrappers) w.unmount()
    wrappers.length = 0
    document.body.innerHTML = ''
  })

  it('按卡片渲染分组：设备数、设备标签与空分组提示', async () => {
    const wrapper = await mountPage()
    const cards = wrapper.findAll('.group-card')
    expect(cards).toHaveLength(2)
    expect(cards[0]!.text()).toContain('A 组')
    expect(cards[0]!.text()).toContain('dev-1')
    expect(cards[1]!.text()).toContain('暂无设备')
  })

  it('分组名称为空时拒绝保存，不调用写接口', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '新建分组')
    // 不填名称直接保存
    await clickButton(wrapper, '保存')

    expect(ElMessage.warning).toHaveBeenCalledWith('请输入分组名称')
    expect(createGroupView).not.toHaveBeenCalled()
  })

  it('新建分组：以空设备列表创建并刷新列表', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '新建分组')
    wrapper.findAllComponents(ElInput)[0]!.vm.$emit('update:modelValue', 'C 组')
    wrapper.findAllComponents(ElInput)[1]!.vm.$emit('update:modelValue', '三号车间')
    await settle()
    await clickButton(wrapper, '保存')

    expect(createGroupView).toHaveBeenCalledOnce()
    expect(vi.mocked(createGroupView).mock.calls[0]![0]).toMatchObject({
      name: 'C 组',
      description: '三号车间',
      device_ids: [],
    })
    expect(ElMessage.success).toHaveBeenCalledWith('已创建')
    expect(listGroupsView).toHaveBeenCalledTimes(2)
  })

  it('编辑分组：保留原有设备列表，改名不能把设备清空', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper.findAll('.group-card')[0]!, '编辑')
    // 表单已回填原值
    expect(wrapper.findAllComponents(ElInput)[0]!.props('modelValue')).toBe('A 组')
    wrapper.findAllComponents(ElInput)[0]!.vm.$emit('update:modelValue', 'A 组（改名）')
    await settle()
    await clickButton(wrapper, '保存')

    expect(updateGroupView).toHaveBeenCalledWith('g-1', {
      name: 'A 组（改名）',
      description: '一号车间',
      color: '#1677ff',
      device_ids: ['dev-1'],
    })
    expect(ElMessage.success).toHaveBeenCalledWith('已更新')
  })

  it('保存失败走统一错误提示，不静默失败', async () => {
    vi.mocked(createGroupView).mockRejectedValue(new Error('duplicate name'))
    const wrapper = await mountPage()
    await clickButton(wrapper, '新建分组')
    wrapper.findAllComponents(ElInput)[0]!.vm.$emit('update:modelValue', 'C 组')
    await settle()
    await clickButton(wrapper, '保存')

    expect(showToast).toHaveBeenCalled()
    const [err, fallback] = showToast.mock.calls[0]!
    expect(fallback).toBe('保存分组失败')
    expect((err as Error).message).toBe('duplicate name')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('管理设备只提交差异：新增调 add、移除调 remove，未变动的设备不碰', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper.findAll('.group-card')[0]!, '管理设备')

    const group = wrapper.findAllComponents(ElCheckboxGroup)
    expect(group).toHaveLength(1)
    expect(wrapper.findAllComponents(ElCheckbox)).toHaveLength(2)
    // 勾选清单：dev-1、dev-2；A 组原含 dev-1 → 改为只保留 dev-2
    group[0]!.vm.$emit('update:modelValue', ['dev-2'])
    await settle()
    await clickButton(wrapper, '保存')

    expect(addDeviceToGroup).toHaveBeenCalledWith('g-1', 'dev-2')
    expect(removeDeviceFromGroup).toHaveBeenCalledWith('g-1', 'dev-1')
    expect(addDeviceToGroup).toHaveBeenCalledTimes(1)
    expect(removeDeviceFromGroup).toHaveBeenCalledTimes(1)
    expect(ElMessage.success).toHaveBeenCalledWith('已更新设备列表')
  })

  it('设备选择未改动时不发起任何写请求', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper.findAll('.group-card')[0]!, '管理设备')
    await clickButton(wrapper, '保存')

    expect(addDeviceToGroup).not.toHaveBeenCalled()
    expect(removeDeviceFromGroup).not.toHaveBeenCalled()
  })

  it('删除需二次确认：取消则一个请求都不发', async () => {
    vi.mocked(confirmDelete).mockResolvedValue(false as never)
    const wrapper = await mountPage()
    await clickButton(wrapper.findAll('.group-card')[1]!, '删除')

    expect(confirmDelete).toHaveBeenCalledWith('B 组')
    expect(deleteGroup).not.toHaveBeenCalled()
  })

  it('确认删除后调用删除接口并刷新', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper.findAll('.group-card')[1]!, '删除')

    expect(deleteGroup).toHaveBeenCalledWith('g-2')
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
    expect(listGroupsView).toHaveBeenCalledTimes(2)
  })

  it('删除失败走统一错误提示', async () => {
    vi.mocked(deleteGroup).mockRejectedValue(new Error('group in use'))
    const wrapper = await mountPage()
    await clickButton(wrapper.findAll('.group-card')[1]!, '删除')

    expect(showToast).toHaveBeenCalled()
    expect(showToast.mock.calls[0]![1]).toBe('删除分组失败')
  })

  it('设备清单接口不可用时分组的展示与操作仍可用（降级为空清单）', async () => {
    vi.mocked(listDevices).mockRejectedValue(new Error('devices 500'))
    const wrapper = await mountPage()

    expect(wrapper.findAll('.group-card')).toHaveLength(2)
    await clickButton(wrapper.findAll('.group-card')[0]!, '管理设备')
    expect(wrapper.findAllComponents(ElCheckbox)).toHaveLength(0)
  })
})
