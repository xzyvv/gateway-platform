import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：点位管理 › 地址偏移（AddressOffset）行为测试。
 *
 * 批量改地址会直接改写现场点位的寄存器地址，错了就是采不到数/采错寄存器。
 * 钉住的核心：
 * - 预览必须与真实下发口径一致：未勾选不改、越界钳到 [0, 65535]；
 * - 一个都没勾选时不发请求；
 * - 确认弹窗取消后一个写请求都不发；
 * - 失败必须可见报错，不能提示「已偏移」。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('@/api/pointsOps', () => ({
  listOffsetPointsView: (...a: unknown[]) => listMock(...a),
  applyOffsetView: (...a: unknown[]) => applyMock(...a),
}))
vi.mock('@/utils/confirm', () => ({ confirmAction: (...a: unknown[]) => confirmMock(...a) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))

const listMock = vi.hoisted(() => vi.fn())
const applyMock = vi.hoisted(() => vi.fn())
const confirmMock = vi.hoisted(() => vi.fn())
const showToast = vi.hoisted(() => vi.fn())

import AddressOffset from './AddressOffset.vue'
import type { OffsetPointViewV } from '@/api/pointsOps'

const POINTS: OffsetPointViewV[] = [
  { id: 'p1', name: '温度', oldAddr: 40001, newAddr: 40001, type: 'int16', selected: false },
  { id: 'p2', name: '电量', oldAddr: 65530, newAddr: 65530, type: 'float', selected: false },
  { id: 'p3', name: '开关', oldAddr: 5, newAddr: 5, type: 'bool', selected: false },
]

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(AddressOffset)
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

async function setOffset(wrapper: ReturnType<typeof mountPage>, value: number) {
  wrapper.findComponent({ name: 'ElInputNumber' }).vm.$emit('update:modelValue', value)
  await settle()
}

async function selectRow(wrapper: ReturnType<typeof mountPage>, index: number) {
  await settle()
  wrapper.findAllComponents({ name: 'ElCheckbox' })[index]!.vm.$emit('update:modelValue', true)
  await settle()
}

describe('AddressOffset 批量地址偏移', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listMock.mockResolvedValue(POINTS.map((p) => ({ ...p })))
    applyMock.mockResolvedValue({ updated: 2 })
    confirmMock.mockResolvedValue(true)
  })

  it('首屏渲染点位与选中计数', async () => {
    const wrapper = mountPage()
    await settle()

    expect(wrapper.findAll('.new')).toHaveLength(3)
    expect(wrapper.text()).toContain('已选 0 个点位')
  })

  it('预览只对勾选项生效：未勾选保持原地址', async () => {
    const wrapper = mountPage()
    await selectRow(wrapper, 0)
    await setOffset(wrapper, 10)

    const previews = wrapper.findAll('.new').map((n) => n.text())
    expect(previews).toEqual(['40011', '65530', '5'])
    expect(wrapper.findAll('.new')[0]!.classes()).toContain('changed')
    expect(wrapper.findAll('.new')[1]!.classes()).not.toContain('changed')
    expect(wrapper.text()).toContain('已选 1 个点位')
    expect(wrapper.findAll('.el-tag').map((t) => t.text())).toContain('+10')
  })

  it('预览越界钳位：上界 65535、下界 0', async () => {
    const wrapper = mountPage()
    await selectRow(wrapper, 1)
    await selectRow(wrapper, 2)
    await setOffset(wrapper, 9999)

    expect(wrapper.findAll('.new').map((n) => n.text())).toEqual(['40001', '65535', '10004'])
    expect(wrapper.findAll('.el-tag').map((t) => t.text())).toContain('+5')

    await setOffset(wrapper, -9999)
    expect(wrapper.findAll('.new').map((n) => n.text())).toEqual(['40001', '55531', '0'])
    // 负向变化用警告色，且带符号
    expect(wrapper.findAll('.el-tag').map((t) => t.text())).toContain('-5')
  })

  it('一个点位都没勾选时拒绝下发', async () => {
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '应用偏移到选中点位').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请先勾选要偏移的点位')
    expect(confirmMock).not.toHaveBeenCalled()
    expect(applyMock).not.toHaveBeenCalled()
  })

  it('确认弹窗取消后一个写请求都不发', async () => {
    confirmMock.mockResolvedValue(false)
    const wrapper = mountPage()
    await selectRow(wrapper, 0)

    await button(wrapper, '应用偏移到选中点位').trigger('click')
    await settle()

    expect(confirmMock).toHaveBeenCalledWith(
      expect.objectContaining({
        title: '批量地址偏移确认',
        message: expect.stringContaining('+10'),
        confirmText: '确认偏移',
      }),
    )
    expect(applyMock).not.toHaveBeenCalled()
  })

  it('确认后按当前偏移量下发并刷新列表', async () => {
    const wrapper = mountPage()
    await selectRow(wrapper, 0)
    await selectRow(wrapper, 2)
    await setOffset(wrapper, 7)

    await button(wrapper, '应用偏移到选中点位').trigger('click')
    await settle()

    expect(applyMock).toHaveBeenCalledWith(expect.any(Array), 7)
    expect(ElMessage.success).toHaveBeenCalledWith('已偏移 2 个点位')
    expect(listMock).toHaveBeenCalledTimes(2)
  })

  it('偏移失败：报出原因，不谎报成功', async () => {
    applyMock.mockRejectedValue(new Error('点位被占用'))
    const wrapper = mountPage()
    await selectRow(wrapper, 0)

    await button(wrapper, '应用偏移到选中点位').trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('点位被占用'), '偏移操作失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(listMock).toHaveBeenCalledTimes(1)
  })

  it('列表加载失败：可见报错，页面不崩', async () => {
    listMock.mockRejectedValue(new Error('list 500'))
    const wrapper = mountPage()
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('list 500'), '加载点位失败')
    expect(wrapper.findAll('.new')).toHaveLength(0)
    expect(button(wrapper, '重新加载').exists()).toBe(true)
  })
})
