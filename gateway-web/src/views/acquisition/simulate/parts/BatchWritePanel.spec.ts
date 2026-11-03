import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage, ElInputNumber, ElSelect } from 'element-plus'

/**
 * D1：点位读写测试 › 批量写入卡片（BatchWritePanel）行为测试。
 *
 * 这里填的是真正要下发到设备的写值，错了会改坏现场设备参数。
 * 钉住的核心：
 * - 行未填全 / 点位重复在本地拦截，绝不带着脏数据下发；
 * - 用户取消确认时一个请求都不发；
 * - 逐行结果按入参顺序回填，成功/失败汇总与操作记录口径一致；
 * - 请求级失败（网络/权限）要给出带处置建议的原文，且不能把行标成成功；
 * - 切换设备后不残留上一台设备的点位与结果。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('@/composables/data/usePointWrite', () => ({
  usePointWrite: () => ({ writeBatch }),
  confirmPointWrite,
}))

const writeBatch = vi.hoisted(() => vi.fn())
const confirmPointWrite = vi.hoisted(() => vi.fn())

import BatchWritePanel from './BatchWritePanel.vue'
import DataTable from '@/components/DataTable.vue'
import type { PointConfig } from '@/api/types'

const POINTS = [
  { id: 'p1', name: '设定温度', data_type: 'float' },
  { id: 'p2', name: '开关状态', data_type: 'bool' },
] as unknown as PointConfig[]
const DEVICES_POINTS = [
  { id: 'p9', name: '另一台设备点位', data_type: 'float' },
] as unknown as PointConfig[]

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPanel(deviceId = 'dev-1', points = POINTS) {
  return mount(BatchWritePanel, { props: { deviceId, points } })
}

function button(wrapper: ReturnType<typeof mountPanel>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim().includes(text))
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

/** 给第 index 行填点位与目标值（表格控件由列定义渲染，按渲染顺序对应行） */
async function fillRow(wrapper: ReturnType<typeof mountPanel>, index: number, pointId: string, value: number) {
  // DataTable 首屏控件要等一个微任务批次才挂载，先落到渲染完成再取控件
  await settle()
  wrapper.findAllComponents(ElSelect)[index]!.vm.$emit('update:modelValue', pointId)
  wrapper.findAllComponents(ElInputNumber)[index]!.vm.$emit('update:modelValue', value)
  await settle()
}

function rows(wrapper: ReturnType<typeof mountPanel>) {
  const table = wrapper.findComponent(DataTable)
  return (table.props() as unknown as { data: Array<Record<string, unknown>> }).data
}

describe('BatchWritePanel 批量写入', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    confirmPointWrite.mockResolvedValue(true)
    writeBatch.mockResolvedValue({ results: [], succeeded: 0, failed: 0, total: 0 })
  })

  it('未选设备时不能提交，选了设备才能点', async () => {
    const noDevice = mountPanel('')
    expect(button(noDevice, '批量写入').attributes('disabled')).toBeDefined()

    const wrapper = mountPanel()
    expect(button(wrapper, '批量写入').attributes('disabled')).toBeUndefined()
  })

  it('点位下拉只列当前设备点位，标签带类型便于确认', async () => {
    const wrapper = mountPanel()
    await settle()
    // 下拉未展开时 ElOption 不挂载，直接读列渲染函数塞进 ElSelect 的选项 vnode
    const options = wrapper.findComponent(ElSelect).vm.$slots.default!()
    const labels = options.map((vnode) => (vnode.props as { label: string }).label)
    expect(labels).toEqual(['设定温度 (float)', '开关状态 (bool)'])
  })

  it('新增/移除行始终至少保留一行', async () => {
    const wrapper = mountPanel()
    expect(rows(wrapper)).toHaveLength(1)

    await button(wrapper, '新增一行').trigger('click')
    expect(rows(wrapper)).toHaveLength(2)

    await wrapper.find('button[aria-label="移除 第 1 行"]').trigger('click')
    expect(rows(wrapper)).toHaveLength(1)

    await wrapper.find('button[aria-label="移除 第 1 行"]').trigger('click')
    expect(rows(wrapper)).toHaveLength(1)
  })

  it('行未填全就提交：本地拦截并提示，不弹确认也不下发', async () => {
    const wrapper = mountPanel()
    await fillRow(wrapper, 0, 'p1', 10)
    await button(wrapper, '新增一行').trigger('click')
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    expect(ElMessage.warning).toHaveBeenCalledWith('请为每一行选择点位并填写有效的数值')
    expect(confirmPointWrite).not.toHaveBeenCalled()
    expect(writeBatch).not.toHaveBeenCalled()
  })

  it('同一点位重复下发被拦下，提示合并为一行', async () => {
    const wrapper = mountPanel()
    await button(wrapper, '新增一行').trigger('click')
    await fillRow(wrapper, 0, 'p1', 10)
    await fillRow(wrapper, 1, 'p1', 20)
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    expect(ElMessage.warning).toHaveBeenCalledWith('点位「p1」在表格中重复，请合并为一行')
    expect(writeBatch).not.toHaveBeenCalled()
  })

  it('用户在确认框取消时一个请求都不发', async () => {
    confirmPointWrite.mockResolvedValue(false)
    const wrapper = mountPanel()
    await fillRow(wrapper, 0, 'p1', 10)
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    expect(confirmPointWrite).toHaveBeenCalledWith([{ id: 'p1', value: 10 }])
    expect(writeBatch).not.toHaveBeenCalled()
    expect(wrapper.find('.batch-result').exists()).toBe(false)
  })

  it('全部成功：按顺序回填每行结果并给出成功汇总', async () => {
    writeBatch.mockResolvedValue({
      results: [
        { id: 'p1', status: 'ok', value: 10 },
        { id: 'p2', status: 'ok', value: 1 },
      ],
      succeeded: 2,
      failed: 0,
      total: 2,
    })
    const wrapper = mountPanel()
    await button(wrapper, '新增一行').trigger('click')
    await fillRow(wrapper, 0, 'p1', 10)
    await fillRow(wrapper, 1, 'p2', 1)
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    expect(writeBatch).toHaveBeenCalledWith([{ id: 'p1', value: 10 }, { id: 'p2', value: 1 }])
    const data = rows(wrapper)
    expect(data[0]!.result).toMatchObject({ status: 'ok', value: 10 })
    expect(data[1]!.result).toMatchObject({ status: 'ok', value: 1 })
    expect(wrapper.find('.batch-result .el-tag').text()).toMatch(/^成功 2 \/ 失败 0（共 2，\d+ms）$/)
    expect(wrapper.find('.batch-result .el-tag').classes()).toContain('el-tag--success')
    expect(wrapper.emitted('log')![0]).toEqual(['success', '批量写入', wrapper.find('.batch-result .el-tag').text()])
  })

  it('部分失败：逐行标明失败原因，汇总降级为警告', async () => {
    writeBatch.mockResolvedValue({
      results: [
        { id: 'p1', status: 'ok', value: 10 },
        { id: 'p2', status: 'error', error: '非法数据值' },
      ],
      succeeded: 1,
      failed: 1,
      total: 2,
    })
    const wrapper = mountPanel()
    await button(wrapper, '新增一行').trigger('click')
    await fillRow(wrapper, 0, 'p1', 10)
    await fillRow(wrapper, 1, 'p2', 1)
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    expect(wrapper.find('.batch-result .el-tag').text()).toMatch(/^成功 1 \/ 失败 1/)
    expect(wrapper.find('.batch-result .el-tag').classes()).toContain('el-tag--warning')
    expect(wrapper.findAll('.batch-error')[0]!.text()).toBe('非法数据值')
    expect(wrapper.emitted('log')![0]![0]).toBe('warning')
  })

  it('全部失败：汇总标红，避免误以为写入成功', async () => {
    writeBatch.mockResolvedValue({
      results: [
        { id: 'p1', status: 'error', error: '设备无响应' },
        { id: 'p2', status: 'error', error: '设备无响应' },
      ],
      succeeded: 0,
      failed: 2,
      total: 2,
    })
    const wrapper = mountPanel()
    await button(wrapper, '新增一行').trigger('click')
    await fillRow(wrapper, 0, 'p1', 10)
    await fillRow(wrapper, 1, 'p2', 1)
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    expect(wrapper.find('.batch-result .el-tag').classes()).toContain('el-tag--danger')
  })

  it('请求级失败：带上处置建议报错，行结果保持未提交', async () => {
    writeBatch.mockResolvedValue({
      results: [],
      succeeded: 0,
      failed: 0,
      total: 2,
      error: '写入超时',
    })
    const wrapper = mountPanel()
    await button(wrapper, '新增一行').trigger('click')
    await fillRow(wrapper, 0, 'p1', 10)
    await fillRow(wrapper, 1, 'p2', 1)
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    const summary = wrapper.find('.batch-result .el-tag').text()
    expect(summary).toContain('批量写入失败：写入超时')
    expect(summary).toContain('（连接超时，请检查设备是否上电')
    expect(wrapper.find('.batch-result .el-tag').classes()).toContain('el-tag--danger')
    expect(rows(wrapper).every((r) => r.result === null)).toBe(true)
    expect(ElMessage.error).toHaveBeenCalledWith(summary)
    expect(wrapper.emitted('log')![0]![0]).toBe('danger')
  })

  it('下发异常时按请求级失败上报，加载态必须复位', async () => {
    writeBatch.mockRejectedValue(new Error('socket hang up'))
    const wrapper = mountPanel()
    await fillRow(wrapper, 0, 'p1', 10)
    await button(wrapper, '批量写入').trigger('click')
    await settle()

    const summary = wrapper.find('.batch-result .el-tag').text()
    expect(summary).toContain('批量写入失败：socket hang up')
    expect(wrapper.find('.batch-result .el-tag').classes()).toContain('el-tag--danger')
    expect(ElMessage.error).toHaveBeenCalledWith(summary)
    expect(wrapper.emitted('log')![0]![0]).toBe('danger')
    expect(button(wrapper, '批量写入').attributes('loading')).toBeUndefined()
  })

  it('切换设备后清空已填点位与上一轮结果', async () => {
    writeBatch.mockResolvedValue({
      results: [{ id: 'p1', status: 'ok', value: 10 }],
      succeeded: 1,
      failed: 0,
      total: 1,
    })
    const wrapper = mountPanel()
    await fillRow(wrapper, 0, 'p1', 10)
    await button(wrapper, '批量写入').trigger('click')
    await settle()
    expect(wrapper.find('.batch-result').exists()).toBe(true)

    await wrapper.setProps({ deviceId: 'dev-2', points: DEVICES_POINTS })
    await settle()

    const data = rows(wrapper)
    expect(data).toHaveLength(1)
    expect(data[0]).toMatchObject({ pointId: '', value: null, result: null })
    expect(wrapper.find('.batch-result').exists()).toBe(false)
  })
})
