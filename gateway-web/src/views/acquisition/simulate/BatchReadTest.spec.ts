import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage, ElOption, ElSelect, ElInputNumber } from 'element-plus'

/**
 * D1：采集 › 读取性能测试（BatchReadTest）行为测试。
 *
 * 这一页用连续 N 次读点统计成功率与 P50/P95/P99，现场据此判断「链路是不是稳」。
 * 钉住的点：统计口径必须与真实读结果一致（不能把失败算成成功、也不能漏算），
 * 低成功率必须给出可执行提示，设备切换后点位列表要跟着换。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})

const h = vi.hoisted(() => ({
  deviceRef: { current: null as unknown as { value: string } },
  readOnce: vi.fn(),
}))

vi.mock('@/composables/acquisition/useDebugTargets', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  if (!h.deviceRef.current) h.deviceRef.current = ref('')
  return { useSharedDebugTarget: () => ({ selectedDeviceId: h.deviceRef.current }) }
})
vi.mock('@/composables/data/useReadPointNow', () => ({
  useReadPointNow: () => ({ readOnce: h.readOnce }),
}))
vi.mock('@/api/devices', () => ({ listDevices: vi.fn() }))
vi.mock('@/api/points', () => ({ listPoints: vi.fn() }))
vi.mock('@/api/protocols', async () => {
  const actual = await vi.importActual<typeof import('@/api/protocols')>('@/api/protocols')
  return { ...actual, listProtocolCapabilities: vi.fn() }
})
const clipboardState = vi.hoisted(() => ({ copyText: vi.fn(async () => true) }))
vi.mock('@/utils/clipboard', () => ({ copyText: clipboardState.copyText }))
const exportState = vi.hoisted(() => ({ csv: vi.fn() }))
vi.mock('@/utils/exportCsv', () => ({
  downloadTableCsv: exportState.csv,
  csvFilename: (prefix: string) => `${prefix}-stamp.csv`,
}))
vi.mock('@/utils/deviceComms', () => ({ recordRead: vi.fn() }))

import BatchReadTest from './BatchReadTest.vue'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { listProtocolCapabilities } from '@/api/protocols'
import { resetProtocolCapsCache } from '@/composables/acquisition/useProtocolCaps'
import { recordRead } from '@/utils/deviceComms'

const DEVICES = [
  { id: 'dev-1', name: '温控器', protocol: 'modbus-tcp' },
  { id: 'dev-2', name: '电表', protocol: 'dlt645' },
]
const POINTS = [
  { id: 'p1', name: '温度', data_type: 'float', device_id: 'dev-1', bus: 'COM1' },
  { id: 'p2', name: '湿度', data_type: 'float', device_id: 'dev-1', bus: 'COM1' },
]
const CAPS = {
  supports_write: true,
  supports_batch_read: true,
  supports_scan_slaves: false,
  supports_frame_extract: true,
  supports_register_scan: false,
  supports_auto_detect: false,
  supports_write_multi: false,
  supports_diag: true,
}

/** 一次读点结果的构造：只关心成败与耗时，其余字段给足避免模板取空 */
function outcome(ok: boolean, latencyMs: number) {
  return {
    pointId: 'p1',
    ok,
    value: ok ? 25.5 : null,
    status: ok ? 'ok' : 'error',
    error: ok ? '' : '采集超时',
    quality: null,
    latencyMs,
    raw: null,
  }
}

/** 每次测试按顺序出队，模拟 N 次连续读的耗时分布 */
function queueReads(list: Array<{ ok: boolean; latencyMs: number }>) {
  const queue = [...list]
  h.readOnce.mockImplementation(async () => {
    const item = queue.shift() ?? { ok: true, latencyMs: 5 }
    return outcome(item.ok, item.latencyMs) as never
  })
}

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(BatchReadTest)
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

/** 选设备 → 选点位 → 设置测试次数，把页面推进到可执行状态 */
async function readyToRun(wrapper: ReturnType<typeof mountPage>, count: number) {
  h.deviceRef.current.value = 'dev-1'
  await settle()
  wrapper.findAllComponents(ElSelect)[1]!.vm.$emit('update:modelValue', 'p1')
  wrapper.findComponent(ElInputNumber).vm.$emit('update:modelValue', count)
  await settle()
}

describe('BatchReadTest 读取性能测试', () => {
  beforeEach(() => {
    // 协议能力是模块级单例：清掉上一条用例灌进去的快照，否则本用例的 mock 返回值不会被重新请求
    resetProtocolCapsCache()
    vi.clearAllMocks()
    h.deviceRef.current.value = ''
    vi.mocked(listDevices).mockResolvedValue(DEVICES as never)
    vi.mocked(listPoints).mockResolvedValue(POINTS as never)
    vi.mocked(listProtocolCapabilities).mockResolvedValue([
      { name: 'modbus-tcp', capabilities: CAPS },
    ] as never)
    queueReads([{ ok: true, latencyMs: 10 }])
  })

  it('挂载后加载设备列表，选项带协议便于现场区分', async () => {
    const wrapper = mountPage()
    await settle()

    const labels = wrapper.findAllComponents(ElOption).map((o) => o.props('label'))
    expect(labels).toEqual(['温控器 (modbus-tcp)', '电表 (dlt645)'])
  })

  it('共享调试目标里的设备已被删除时清空选择，不展示陈旧 id', async () => {
    h.deviceRef.current.value = 'dev-gone'
    const wrapper = mountPage()
    await settle()

    expect(h.deviceRef.current.value).toBe('')
    expect(wrapper.findAllComponents(ElSelect)[0]!.props('modelValue')).toBe('')
  })

  it('选中设备后加载该设备点位，未选设备时点位下拉不可用', async () => {
    const wrapper = mountPage()
    await settle()
    expect(wrapper.findAllComponents(ElSelect)[1]!.props('disabled')).toBe(true)

    h.deviceRef.current.value = 'dev-1'
    await nextTick()
    await settle()

    expect(listPoints).toHaveBeenCalledWith('dev-1')
    expect(wrapper.findAllComponents(ElSelect)[1]!.props('disabled')).toBe(false)
    const pointLabels = wrapper.findAllComponents(ElOption).map((o) => o.props('label'))
    expect(pointLabels).toContain('温度 (float)')
  })

  it('未选点位时「开始测试」禁用，选中后可点', async () => {
    const wrapper = mountPage()
    await settle()
    expect(button(wrapper, '开始测试').attributes('disabled')).toBeDefined()

    const pointSelect = wrapper.findAllComponents(ElSelect)[1]!
    pointSelect.vm.$emit('update:modelValue', 'p1')
    await settle()
    expect(button(wrapper, '开始测试').attributes('disabled')).toBeUndefined()
  })

  it('全部成功：按真实耗时算出 P50/P95/P99 与成功率', async () => {
    const wrapper = mountPage()
    await settle()
    await readyToRun(wrapper, 4)

    queueReads([
      { ok: true, latencyMs: 10 },
      { ok: true, latencyMs: 20 },
      { ok: true, latencyMs: 30 },
      { ok: true, latencyMs: 40 },
    ])
    await button(wrapper, '开始测试').trigger('click')
    await settle()

    expect(h.readOnce).toHaveBeenCalledTimes(4)
    expect(h.readOnce.mock.calls[0]![0]).toBe('p1')
    expect(wrapper.find('.result-summary .el-tag').text()).toBe('成功率 100%')
    expect(wrapper.find('.result-meta').text()).toBe('4 成功 / 0 失败 / 共 4 次')
    // P50=30 / P95=40 / 最小 10 / 平均 25 / 最大 40（按同一份耗时数组计算）
    const block = wrapper.find('.result-block').text()
    for (const t of ['P50', '30ms', 'P95', '40ms', '25ms', '10ms']) expect(block).toContain(t)
    expect(wrapper.find('.result-summary .el-tag').classes()).toContain('el-tag--success')
    expect(ElMessage.warning).not.toHaveBeenCalled()
  })

  it('半数失败：成功率如实折半并给出可执行提示', async () => {
    const wrapper = mountPage()
    await settle()
    await readyToRun(wrapper, 4)

    queueReads([
      { ok: true, latencyMs: 10 },
      { ok: false, latencyMs: 10 },
      { ok: true, latencyMs: 10 },
      { ok: false, latencyMs: 10 },
    ])
    await button(wrapper, '开始测试').trigger('click')
    await settle()

    expect(wrapper.find('.result-summary .el-tag').text()).toBe('成功率 50%')
    expect(wrapper.find('.result-meta').text()).toBe('2 成功 / 2 失败 / 共 4 次')
    expect(wrapper.find('.result-summary .el-tag').classes()).toContain('el-tag--danger')
    expect(ElMessage.warning).toHaveBeenCalledWith('成功率仅 50%，请检查链路')
  })

  it('成功率 80% 时只标黄，不误报「请检查链路」', async () => {
    const wrapper = mountPage()
    await settle()
    await readyToRun(wrapper, 5)

    queueReads([
      { ok: true, latencyMs: 10 },
      { ok: true, latencyMs: 10 },
      { ok: true, latencyMs: 10 },
      { ok: true, latencyMs: 10 },
      { ok: false, latencyMs: 10 },
    ])
    await button(wrapper, '开始测试').trigger('click')
    await settle()

    expect(wrapper.find('.result-summary .el-tag').classes()).toContain('el-tag--warning')
    expect(ElMessage.warning).not.toHaveBeenCalled()
  })

  it('每次读取都记入设备与总线统计，失败带 read_error 标记', async () => {
    const wrapper = mountPage()
    await settle()
    await readyToRun(wrapper, 2)

    queueReads([
      { ok: true, latencyMs: 10 },
      { ok: false, latencyMs: 20 },
    ])
    await button(wrapper, '开始测试').trigger('click')
    await settle()

    expect(recordRead).toHaveBeenCalledTimes(2)
    expect(vi.mocked(recordRead).mock.calls[0]).toEqual(['dev-1', true, 10])
  })

  it('测试日志倒序追加：最新的完成结论在最上方', async () => {
    const wrapper = mountPage()
    await settle()
    await readyToRun(wrapper, 1)

    await button(wrapper, '开始测试').trigger('click')
    await settle()

    const logs = wrapper.findAll('.log-item')
    expect(logs).toHaveLength(2)
    expect(logs[0]!.text()).toContain('成功率=')
    expect(logs[1]!.text()).toContain('发起 1 次读取')
  })

  /**
   * 测试日志此前只能「清空」：一轮压测跑完，P50/P99 的结论要交给别人看，
   * 只能截图。现在与另外两个日志面板共用 LogPanel 的三件套。
   *
   * 变异验证：删掉 LogPanel 上的 copyable/exportable → 本用例红（按钮找不到）。
   */
  it('测试日志可复制全部、可导出 CSV', async () => {
    const wrapper = mountPage()
    await settle()
    await readyToRun(wrapper, 1)

    await button(wrapper, '开始测试').trigger('click')
    await settle()
    expect(wrapper.findAll('.log-item')).toHaveLength(2)

    const copyBtn = wrapper.findAll('button').find((b) => b.text().includes('复制全部'))!
    await copyBtn.trigger('click')
    expect(clipboardState.copyText).toHaveBeenCalledTimes(1)
    const [text, okMessage] = clipboardState.copyText.mock.calls[0] as unknown as [string, string]
    // 倒序的日志翻回时间正序，「发起」应排在「完成」之前
    expect(text.indexOf('发起')).toBeLessThan(text.indexOf('成功率='))
    expect(okMessage).toBe('已复制 2 条测试日志')

    const exportBtn = wrapper.findAll('button').find((b) => b.text().includes('导出 CSV'))!
    await exportBtn.trigger('click')
    const [header, rows, filename] = exportState.csv.mock.calls[0] as [string[], unknown[][], string]
    expect(header).toEqual(['时间', '类型', '标签', '内容'])
    expect(rows).toHaveLength(2)
    expect(filename).toBe('batch-read-log-stamp.csv')
  })

  it('没有结果时「清空结果」禁用，有结果后清空会移除结果块', async () => {
    const wrapper = mountPage()
    await settle()
    expect(button(wrapper, '清空结果').attributes('disabled')).toBeDefined()

    h.deviceRef.current.value = 'dev-1'
    await settle()
    wrapper.findAllComponents(ElSelect)[1]!.vm.$emit('update:modelValue', 'p1')
    await settle()
    await button(wrapper, '开始测试').trigger('click')
    await settle()
    expect(wrapper.find('.result-block').exists()).toBe(true)

    await button(wrapper, '清空结果').trigger('click')
    await settle()
    expect(wrapper.find('.result-block').exists()).toBe(false)
  })

  it('清空测试日志后回到空态提示', async () => {
    const wrapper = mountPage()
    await settle()
    h.deviceRef.current.value = 'dev-1'
    await settle()
    wrapper.findAllComponents(ElSelect)[1]!.vm.$emit('update:modelValue', 'p1')
    await settle()
    await button(wrapper, '开始测试').trigger('click')
    await settle()
    expect(wrapper.findAll('.log-item')).toHaveLength(2)

    await button(wrapper, '清空').trigger('click')
    await settle()
    expect(wrapper.findAll('.log-item')).toHaveLength(0)
    expect(wrapper.text()).toContain('执行连续读取后，每次读取的结果会显示在这里')
  })

  it('设备列表加载失败不崩页：设备下拉为空、加载态复位', async () => {
    vi.mocked(listDevices).mockRejectedValue(new Error('network down'))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.exists()).toBe(true)
    expect(wrapper.findAllComponents(ElOption)).toHaveLength(0)
    expect(wrapper.findAllComponents(ElSelect)[0]!.props('loading')).toBe(false)
  })

  it('点位列表加载失败时点位下拉为空，开始测试仍保持禁用', async () => {
    vi.mocked(listPoints).mockRejectedValue(new Error('points 500'))
    const wrapper = mountPage()
    await settle()
    h.deviceRef.current.value = 'dev-1'
    await nextTick()
    await settle()

    expect(wrapper.findAllComponents(ElOption).map((o) => o.props('label'))).toEqual(['温控器 (modbus-tcp)', '电表 (dlt645)'])
    expect(button(wrapper, '开始测试').attributes('disabled')).toBeDefined()
  })

  /**
   * 复用「点位读写」页的 DevicePointSelector（本页此前内联复制了一份下拉）。
   * 除标签格式/aria 口径统一外，最实质的差别是带上了协议能力徽标——
   * 「这个协议到底支不支持批量读」直接决定本页的测试结论有没有意义。
   */
  it('复用统一选择器：按协议能力声明渲染能力徽标', async () => {
    const wrapper = mountPage()
    await settle()
    h.deviceRef.current.value = 'dev-1'
    await settle()

    expect(listProtocolCapabilities).toHaveBeenCalled()
    const text = wrapper.text()
    expect(text).toContain('支持批量读')
    expect(text).toContain('支持写入')
    expect(text).toContain('不支持扫描')
  })

  it('切换点位清空上一轮统计，避免把旧点位的成绩算到新点位上', async () => {
    const wrapper = mountPage()
    await settle()
    await readyToRun(wrapper, 1)
    await button(wrapper, '开始测试').trigger('click')
    await settle()
    expect(wrapper.find('.result-block').exists()).toBe(true)

    // 模拟用户在下拉里换点位（Element Plus 同时发出 update:modelValue 与 change）
    const pointSelect = wrapper.findAllComponents(ElSelect)[1]!
    pointSelect.vm.$emit('update:modelValue', 'p2')
    pointSelect.vm.$emit('change', 'p2')
    await settle()

    expect(wrapper.find('.result-block').exists()).toBe(false)
  })

  it('快速切换设备时丢弃过期的点位响应（Bug 181）', async () => {
    const wrapper = mountPage()
    await settle()

    // dev-1 的请求挂起，随后切到 dev-2 并让它先返回
    let releaseDev1: (() => void) | null = null
    vi.mocked(listPoints).mockImplementation((id: string) =>
      id === 'dev-1'
        ? new Promise((resolve) => {
            releaseDev1 = () => resolve([{ id: 'p-old', name: '旧点位', data_type: 'float' }] as never)
          })
        : Promise.resolve([{ id: 'p-new', name: '新点位', data_type: 'float' }] as never),
    )

    h.deviceRef.current.value = 'dev-1'
    await settle()
    h.deviceRef.current.value = 'dev-2'
    await settle()
    expect(wrapper.findAllComponents(ElOption).map((o) => o.props('label'))).toContain('新点位 (float)')

    // 旧设备的响应姗姗来迟：不得覆盖当前设备已加载的点位
    releaseDev1!()
    await settle()
    const labels = wrapper.findAllComponents(ElOption).map((o) => o.props('label'))
    expect(labels).toContain('新点位 (float)')
    expect(labels).not.toContain('旧点位 (float)')
  })
})
