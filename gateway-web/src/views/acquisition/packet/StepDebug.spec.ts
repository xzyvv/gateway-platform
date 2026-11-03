import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

vi.mock('@/api/protocolTools', () => ({
  listPresets: vi.fn(),
  sendFrame: vi.fn(),
}))
vi.mock('@/api/http', () => ({ abortAllInflight: vi.fn() }))
vi.mock('@/composables/acquisition/useDebugTargets', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  const selectedBus = ref('/dev/ttyUSB0')
  return {
    useSharedDebugTarget: () => ({ selectedBus, selectedDeviceId: ref('') }),
    useDebugTargets: () => ({
      buses: ref([{ port_name: '/dev/ttyUSB0', protocol: 'modbus' }]),
      busOptions: ref([{ label: '调试总线', value: '/dev/ttyUSB0' }]),
      loading: ref(false),
      selectedBus,
    }),
  }
})
vi.mock('@/utils/confirm', () => ({
  confirmAction: vi.fn(),
  confirmDelete: vi.fn(),
}))
// 只替换消息条，保留真实表单组件：断言「失败即停」是否真的提示到用户
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})

import StepDebug from './StepDebug.vue'
import { listPresets, sendFrame, type SendResult } from '@/api/protocolTools'
import { confirmAction } from '@/utils/confirm'
import { ElMessage } from 'element-plus'
import { makeDefaultSequence, useStepSequences } from '@/composables/acquisition/useStepSequences'
import { scopedKey } from '@/utils/gatewayStorage'

/**
 * 单步调试：这是「把多条帧按顺序打到现场总线」的执行器。
 * 最怕的两件事：
 *   1. 某一步失败后还继续下发，错误序列继续占用总线（失败即停必须是真的）；
 *   2. 序列/步骤只在内存里，刷新后消失，现场重填一遍 HEX 风险极高。
 */

const SEQ_KEY = scopedKey('gw_step_debug_sequences')

const presets = [
  { name: '读保持寄存器 1', hex: '01 03 00 00 00 01' },
  { name: '读输入寄存器 2', hex: '01 04 00 00 00 01' },
]

const okResult: SendResult = {
  requestHex: '01 03 00 00 00 01',
  responseHex: '01 03 02 00 2A',
  success: true,
  latencyMs: 8,
  parsed: '值 42',
  error: null,
}

const wrappers: Array<ReturnType<typeof mount>> = []

async function settle() {
  await flushPromises()
  await nextTick()
}

/** 序列是模块级单例，必须每个用例都复位，否则上一条用例的步骤会漏到下一条 */
function resetSequences() {
  const { sequences, activeId } = useStepSequences()
  const seq = makeDefaultSequence()
  sequences.value = [seq]
  activeId.value = seq.id
}

async function mountPage() {
  const wrapper = mount(StepDebug)
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

function button(wrapper: ReturnType<typeof mount>, text: string) {
  const hit = wrapper.findAll('button').find((b) => b.text() === text)
  expect(hit, `找不到按钮「${text}」`).toBeTruthy()
  return hit!
}

function stepItems(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAll('.step-item')
}

describe('StepDebug 单步调试', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
    resetSequences()
    vi.mocked(listPresets).mockResolvedValue(presets)
    vi.mocked(sendFrame).mockResolvedValue(okResult)
    vi.mocked(confirmAction).mockResolvedValue(true)
  })

  afterEach(() => {
    for (const w of wrappers) w.unmount()
    wrappers.length = 0
  })

  it('新增步骤后可编辑名称/HEX，并出现在序列与持久化数据里', async () => {
    const wrapper = await mountPage()
    expect(stepItems(wrapper)).toHaveLength(2)

    await button(wrapper, '添加步骤').trigger('click')
    await settle()
    expect(stepItems(wrapper)).toHaveLength(3)

    // 新步骤默认空 HEX；编辑成合法帧后才会被持久化保留（非法步骤会被丢弃）
    await stepItems(wrapper)[2]!.find('.step-name input').setValue('自定义帧')
    await stepItems(wrapper)[2]!.find('.step-hex input').setValue('AA BB')
    await settle()

    expect((stepItems(wrapper)[2]!.find('.step-name input').element as HTMLInputElement).value).toBe('自定义帧')
    expect((stepItems(wrapper)[2]!.find('.step-hex input').element as HTMLInputElement).value).toBe('AA BB')

    const stored = JSON.parse(localStorage.getItem(SEQ_KEY) ?? '[]') as Array<{ steps: Array<{ name: string; hex: string }> }>
    expect(stored[0]!.steps).toHaveLength(3)
    expect(stored[0]!.steps[2]!.name).toBe('自定义帧')
    expect(stored[0]!.steps[2]!.hex).toBe('AA BB')
  })

  it('删除步骤后序列减少，持久化里也不再有它', async () => {
    const wrapper = await mountPage()

    await stepItems(wrapper)[1]!.findAll('.step-tools button').find((b) => b.text() === '删除')!.trigger('click')
    await settle()

    expect(stepItems(wrapper)).toHaveLength(1)
    const stored = JSON.parse(localStorage.getItem(SEQ_KEY) ?? '[]') as Array<{ steps: unknown[] }>
    expect(stored[0]!.steps).toHaveLength(1)
  })

  it('逐步下发严格按序列顺序执行，每步完成后更新进度与状态', async () => {
    const wrapper = await mountPage()

    await button(wrapper, '单步执行').trigger('click')
    await settle()
    expect(vi.mocked(sendFrame).mock.calls[0]![0]).toBe(presets[0]!.hex)
    expect(wrapper.find('.seq-meta .el-tag').text()).toBe('完成 1/2')
    expect(stepItems(wrapper)[0]!.find('.el-tag').text()).toBe('成功')

    await button(wrapper, '单步执行').trigger('click')
    await settle()
    expect(vi.mocked(sendFrame).mock.calls[1]![0]).toBe(presets[1]!.hex)
    expect(wrapper.find('.seq-meta .el-tag').text()).toBe('完成 2/2')
    expect(stepItems(wrapper)[1]!.find('.el-tag').text()).toBe('成功')

    // 顺序钉子：两次下发的帧必须与序列顺序一致，不能因为异步而倒序
    expect(vi.mocked(sendFrame).mock.calls.map((c) => c[0])).toEqual([presets[0]!.hex, presets[1]!.hex])
    // 每一步都走真实总线与超时参数
    expect(vi.mocked(sendFrame).mock.calls.every((c) => c[1] === '/dev/ttyUSB0' && c[2] === 1500)).toBe(true)
  })

  it('某一步失败后按「失败即停」终止，后续步骤不再下发到总线', async () => {
    vi.mocked(listPresets).mockResolvedValue([
      { name: '步骤1', hex: '01 03 00 00 00 01' },
      { name: '步骤2', hex: '01 03 00 00 00 02' },
      { name: '步骤3', hex: '01 03 00 00 00 03' },
    ])
    let calls = 0
    vi.mocked(sendFrame).mockImplementation(async (hex: string) => {
      calls += 1
      if (calls === 2) {
        return { requestHex: hex, responseHex: '', success: false, latencyMs: 0, parsed: '', error: 'timeout' }
      }
      return { requestHex: hex, responseHex: '01 03 02 00 2A', success: true, latencyMs: 8, parsed: '值 42', error: null }
    })
    const wrapper = await mountPage()
    expect(stepItems(wrapper)).toHaveLength(3)

    await button(wrapper, '自动执行剩余').trigger('click')
    // 第 1 步成功后有 120ms 节流，用 waitFor 等第 2 步失败落定
    await vi.waitFor(() => expect(vi.mocked(sendFrame)).toHaveBeenCalledTimes(2), { timeout: 2000 })
    await settle()

    // 防现场事故：第 3 步绝不能再打到总线（错误序列继续下发会误写设备）
    expect(vi.mocked(sendFrame)).toHaveBeenCalledTimes(2)
    expect(vi.mocked(sendFrame).mock.calls.map((c) => c[0])).not.toContain('01 03 00 00 00 03')
    expect(vi.mocked(ElMessage.warning)).toHaveBeenCalledWith('第 2 步失败，已按「失败即停」停止')

    expect(stepItems(wrapper)[1]!.find('.el-tag').text()).toBe('失败')
    expect(stepItems(wrapper)[2]!.find('.el-tag').exists()).toBe(false)
    expect(wrapper.find('.seq-meta .el-tag').text()).toBe('完成 2/3')
    // 日志区保留失败原因供取证
    expect(wrapper.text()).toContain('超时无响应')
  })

  it('序列写入 localStorage，重新挂载（模拟刷新）后能恢复步骤', async () => {
    const wrapper = await mountPage()
    await button(wrapper, '添加步骤').trigger('click')
    await settle()
    await stepItems(wrapper)[2]!.find('.step-name input').setValue('现场专用帧')
    await stepItems(wrapper)[2]!.find('.step-hex input').setValue('AA BB CC')
    await settle()

    const stored = JSON.parse(localStorage.getItem(SEQ_KEY) ?? '[]') as Array<{ steps: Array<{ hex: string }> }>
    expect(stored[0]!.steps.map((s) => s.hex)).toContain('AA BB CC')
    wrapper.unmount()

    // 模拟刷新：清掉模块级单例缓存后重新 import，让组件只能从 localStorage 恢复
    vi.resetModules()
    const { default: FreshStepDebug } = await import('./StepDebug.vue')
    const freshApi = await import('@/api/protocolTools')
    vi.mocked(freshApi.listPresets).mockResolvedValue([])
    vi.mocked(freshApi.sendFrame).mockResolvedValue(okResult)

    const fresh = mount(FreshStepDebug)
    wrappers.push(fresh)
    await settle()

    expect(stepItems(fresh)).toHaveLength(3)
    expect((stepItems(fresh)[2]!.find('.step-hex input').element as HTMLInputElement).value).toBe('AA BB CC')
  })

  it('删空序列后步骤列表显示空态，持久化数据同步清空', async () => {
    const wrapper = await mountPage()

    // 步骤数动态变化，逐次删最后一个
    while (stepItems(wrapper).length > 0) {
      const items = stepItems(wrapper)
      await items[items.length - 1]!.findAll('.step-tools button').find((b) => b.text() === '删除')!.trigger('click')
      await settle()
    }

    expect(stepItems(wrapper)).toHaveLength(0)
    expect(wrapper.text()).toContain('添加步骤，或从预置添加一条开始')
    const stored = JSON.parse(localStorage.getItem(SEQ_KEY) ?? '[]') as Array<{ steps: unknown[] }>
    expect(stored[0]!.steps).toHaveLength(0)
  })
})
