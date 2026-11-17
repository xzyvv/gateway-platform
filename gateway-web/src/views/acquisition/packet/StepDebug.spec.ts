import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { KeepAlive, defineComponent, h, nextTick, ref } from 'vue'

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
// 执行日志导出：文件名时间戳用固定值，断言不受运行机器时区影响
const exportState = vi.hoisted(() => ({ csv: vi.fn() }))
vi.mock('@/utils/exportCsv', () => ({
  downloadTableCsv: exportState.csv,
  csvFilename: (prefix: string) => `${prefix}-stamp.csv`,
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
import { abortAllInflight } from '@/api/http'
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

/**
 * 把 StepDebug 放进 <KeepAlive> 里挂载，模拟它在 ProtocolConsoleHub 中的真实位置。
 * `show=false` 是「切到别的页签」（走 deactivated，不卸载），`show=true` 是切回。
 */
async function mountInKeepAlive() {
  const show = ref(true)
  const Host = defineComponent({
    setup() {
      return () => h(KeepAlive, null, { default: () => (show.value ? h(StepDebug) : null) })
    },
  })
  const wrapper = mount(Host)
  wrappers.push(wrapper)
  await settle()
  return { wrapper, show }
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

  /**
   * 执行日志必须能整体导出为 CSV。
   *
   * 改造前这块面板只有「复制全部 / 清空」：要按耗时排序分析一次 20 步的长序列，
   * 只能一条条点「复制」再手工拼。导出走统一出口（公式注入防护 + BOM）。
   *
   * 变异验证：删掉 StepRunLogPanel 上的 @export → 本用例红（downloadTableCsv 未被调用）。
   */
  it('执行日志可整体导出为 CSV，行数与日志条数一致', async () => {
    const wrapper = await mountPage()
    await button(wrapper, '单步执行').trigger('click')
    await settle()
    // 再跑一步，确保导出的是「全部」而不是最后一条
    await button(wrapper, '单步执行').trigger('click')
    await settle()

    const exportBtn = wrapper.findAll('button').find((b) => b.text().includes('导出 CSV'))
    expect(exportBtn, '执行日志面板应提供「导出 CSV」').toBeTruthy()
    await exportBtn!.trigger('click')

    expect(exportState.csv).toHaveBeenCalledTimes(1)
    const [header, rows, filename] = exportState.csv.mock.calls[0] as [string[], unknown[][], string]
    expect(header).toContain('请求HEX')
    expect(rows).toHaveLength(2)
    expect(filename).toBe('step-debug-log-stamp.csv')
    expect(vi.mocked(ElMessage.success)).toHaveBeenCalledWith('已导出 2 条执行日志')
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

  /**
   * KeepAlive 场景（本页在 ProtocolConsoleHub 的 <KeepAlive> 内）。
   *
   * 切页签只触发 onDeactivated、不触发 onUnmounted，因此全局 Ctrl+Z 监听必须随
   * deactivate 摘掉：否则在「报文捕获」页按 Ctrl+Z 会命中本页撤销逻辑，静默改坏
   * 一条用户根本没在看的序列。
   */
  it('KeepAlive 切走时摘掉全局快捷键：页外按 Ctrl+Z 不改序列', async () => {
    const { wrapper, show } = await mountInKeepAlive()
    // 必须先制造可撤销的结构变更，否则 onUndo 会因 canUndo=false 提前 return，
    // 断言就成了「什么都没发生」的空转。
    await button(wrapper, '添加步骤').trigger('click')
    await settle()
    expect(stepItems(wrapper)).toHaveLength(3)

    show.value = false // 切到别的页签：走 deactivated，不卸载
    await settle()

    // 页外按键：监听若仍挂着，这里会把序列撤回到 2 条
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', ctrlKey: true }))
    await settle()

    show.value = true
    await settle()
    expect(stepItems(wrapper)).toHaveLength(3)
  })

  /**
   * 自动执行是「按顺序把帧打到真实设备」，用户切走后若继续跑，现场只看到设备
   * 莫名被写，界面上也没有「仍在执行」的提示、无从点「中止」。
   */
  it('KeepAlive 切走时停止自动执行：后续步骤不再打到总线', async () => {
    vi.mocked(listPresets).mockResolvedValue([
      { name: '步骤1', hex: '01 03 00 00 00 01' },
      { name: '步骤2', hex: '01 03 00 00 00 02' },
      { name: '步骤3', hex: '01 03 00 00 00 03' },
    ])
    // 第 2 步挂在总线上不返回，制造「执行途中切页签」的时间窗
    let releaseStep2: (() => void) | null = null
    vi.mocked(sendFrame).mockImplementation(async (hex: string) => {
      if (hex === '01 03 00 00 00 02') {
        await new Promise<void>((res) => { releaseStep2 = res })
      }
      return okResult
    })

    const { wrapper, show } = await mountInKeepAlive()
    expect(stepItems(wrapper)).toHaveLength(3)

    await button(wrapper, '自动执行剩余').trigger('click')
    await vi.waitFor(() => expect(vi.mocked(sendFrame)).toHaveBeenCalledTimes(2), { timeout: 2000 })
    expect(releaseStep2).toBeTruthy()

    show.value = false
    await settle()

    // 解除第 2 步阻塞：循环应在当前步返回后跳出，第 3 步不得再打到总线
    releaseStep2!()
    await vi.waitFor(
      () => expect(ElMessage.info).toHaveBeenCalledWith('已中止，未执行的步骤保留为待执行'),
      { timeout: 2000 },
    )
    expect(vi.mocked(sendFrame)).toHaveBeenCalledTimes(2)
    expect(vi.mocked(sendFrame).mock.calls.map((c) => c[0])).not.toContain('01 03 00 00 00 03')

    // 有意**不**在 deactivate 时调全局 abortAllInflight（那是切网关/登出用的核选项，
    // 会把其它页签正在跑的请求一起打断）。这条断言把这个决定钉住，防止日后被"顺手补上"。
    expect(vi.mocked(abortAllInflight)).not.toHaveBeenCalled()
  })

  it('切回页签后全局快捷键恢复生效（撤销能改序列）', async () => {
    const { wrapper, show } = await mountInKeepAlive()
    await button(wrapper, '添加步骤').trigger('click')
    await settle()
    expect(stepItems(wrapper)).toHaveLength(3)

    show.value = false
    await settle()
    show.value = true
    await settle()

    // 若 onActivated 漏了重新挂载，监听会永久失效——用户按 Ctrl+Z 毫无反应
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'z', ctrlKey: true }))
    await settle()
    expect(stepItems(wrapper)).toHaveLength(2)
  })
})
