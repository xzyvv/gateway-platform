import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：设备管理 › 批量导入 · 设备分支（DeviceImportBranch）行为测试。
 *
 * 这条链路会批量创建真实设备（每条都会落盘并热重载配置），出错代价高。
 * 钉住的核心：
 * - 四步流程能从「下载模板」一路走到「确认导入」（早期版本在下模板步没有出路）；
 * - CSV 的本地校验结论必须如实反映（缺字段、ID 重复、模板不存在/协议不符）；
 * - 逐行下发后成功/失败要逐条回填，不能把失败算成成功，历史记录口径一致。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))
vi.mock('@/utils/batchImportCsv', async () => {
  const actual = await vi.importActual<typeof import('@/utils/batchImportCsv')>('@/utils/batchImportCsv')
  return { ...actual, downloadTemplate: vi.fn() }
})
vi.mock('@/api/devices', () => ({ createDevice: vi.fn() }))

const showToast = vi.hoisted(() => vi.fn())

import DeviceImportBranch from './DeviceImportBranch.vue'
import DataTable from '@/components/DataTable.vue'
import ImportFileUpload from './ImportFileUpload.vue'
import ImportValidatePreview from './ImportValidatePreview.vue'
import ImportResultPanel from './ImportResultPanel.vue'
import { createDevice } from '@/api/devices'
import { downloadTemplate } from '@/utils/batchImportCsv'

const HEADER = '设备名称,设备ID,所属总线,协议,从站ID,模板,地址'
const TEMPLATES = [{ name: '温控模板', protocol: 'modbus-tcp' }]

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage(templates = TEMPLATES) {
  return mount(DeviceImportBranch, { props: { templates } })
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

/** 读到校验预览组件的入参（DataTable 是泛型组件，直接 props(key) 拿不到类型） */
function previewProps(wrapper: ReturnType<typeof mountPage>) {
  return wrapper.findComponent(ImportValidatePreview).props() as unknown as {
    total: number
    valid: number
    invalid: number
    rows: Array<{ name: string; valid: boolean; error?: string }>
  }
}

function tableData(wrapper: ReturnType<typeof mountPage>, which: 'first' | 'last' = 'first') {
  const tables = wrapper.findAllComponents(DataTable)
  const table = which === 'first' ? tables[0]! : tables.at(-1)!
  return (table.props() as unknown as { data: Array<Record<string, unknown>> }).data
}

/** 从「下载模板」步进入「上传文件」步 */
async function gotoUpload(wrapper: ReturnType<typeof mountPage>) {
  await button(wrapper, '下一步').trigger('click')
  await settle()
  expect(wrapper.findComponent(ImportFileUpload).exists()).toBe(true)
}

/** 选择 CSV 文件并把内容读进组件（FileReader 是异步的，用轮询等结果） */
async function uploadCsv(wrapper: ReturnType<typeof mountPage>, csv: string) {
  const file = new File([csv], 'devices.csv', { type: 'text/csv' })
  wrapper.findComponent(ImportFileUpload).vm.$emit('file-change', { name: 'devices.csv', raw: file })
  await vi.waitFor(() =>
    expect(wrapper.findComponent(ImportFileUpload).props('rawText')).toContain('设备名称'),
  )
  await settle()
}

/** 上传 → 校验预览步（预览组件只在这一步渲染） */
async function toPreviewStep(wrapper: ReturnType<typeof mountPage>, csv: string) {
  await gotoUpload(wrapper)
  await uploadCsv(wrapper, csv)
  await button(wrapper, '下一步').trigger('click')
  await settle()
}

/** 上传 → 校验预览 → 确认步 */
async function toConfirmStep(wrapper: ReturnType<typeof mountPage>, csv: string) {
  await toPreviewStep(wrapper, csv)
  await button(wrapper, '下一步').trigger('click')
  await settle()
}

describe('DeviceImportBranch 设备批量导入', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(createDevice).mockResolvedValue(undefined as never)
  })

  it('下载模板后可以继续进入上传步（流程不能停在第一步）', async () => {
    const wrapper = mountPage()
    await button(wrapper, '下载 CSV 模板').trigger('click')
    expect(downloadTemplate).toHaveBeenCalledOnce()
    expect(ElMessage.success).toHaveBeenCalledWith('模板已开始下载')

    await gotoUpload(wrapper)
    expect(wrapper.findComponent(ImportFileUpload).props('rawText')).toBe('')
    // 未上传文件时不能进入校验预览
    expect(wrapper.findAll('button').some((b) => b.text().trim() === '下一步')).toBe(false)
  })

  it('上传 CSV 后读入内容并给出前 5 行预览', async () => {
    const wrapper = mountPage()
    await gotoUpload(wrapper)
    await uploadCsv(wrapper, [HEADER, 'a,dev_a,TCP-1,modbus-tcp,1,温控模板,', 'b,dev_b,TCP-1,modbus-tcp,2,温控模板,'].join('\n'))

    const upload = wrapper.findComponent(ImportFileUpload)
    expect(upload.props('previewLines').split('\n')).toHaveLength(3)
    expect(button(wrapper, '下一步').exists()).toBe(true)
  })

  it('校验预览如实统计有效/无效行，并逐行给出无效原因', async () => {
    const wrapper = mountPage()
    await toPreviewStep(
      wrapper,
      [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,1,温控模板,', ',dev_x,TCP-1,modbus-tcp,3,温控模板,'].join('\n'),
    )

    const preview = previewProps(wrapper)
    expect(preview.total).toBe(2)
    expect(preview.valid).toBe(1)
    expect(preview.invalid).toBe(1)
    expect(preview.rows[1]).toMatchObject({ valid: false })
    expect(preview.rows[1]!.error).toContain('缺少设备名称')
  })

  it('设备 ID 重复在本地就被拦下', async () => {
    const wrapper = mountPage()
    await toPreviewStep(
      wrapper,
      [HEADER, '温控器,dev_dup,TCP-1,modbus-tcp,1,温控模板,', '电表,dev_dup,TCP-1,modbus-tcp,2,温控模板,'].join('\n'),
    )

    const rows = previewProps(wrapper).rows
    expect(rows[1]!.error).toContain('设备ID「dev_dup」重复')
    expect(previewProps(wrapper).valid).toBe(1)
  })

  it('模板库中不存在的模板：报错里列出可用模板，避免反复试错', async () => {
    const wrapper = mountPage()
    await toPreviewStep(wrapper, [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,1,未知模板,'].join('\n'))

    const rows = previewProps(wrapper).rows
    expect(rows[0]!.error).toContain('模板「未知模板」不存在，可用模板：温控模板')
    expect(previewProps(wrapper).valid).toBe(0)
  })

  it('模板协议与填写协议不一致时明确指出差异', async () => {
    const wrapper = mountPage()
    await toPreviewStep(wrapper, [HEADER, '电表,dev_meter,TCP-1,dlt645,0,温控模板,1234567890'].join('\n'))

    const rows = previewProps(wrapper).rows
    expect(rows[0]!.error).toContain('模板「温控模板」的协议是 modbus-tcp，与填写的 dlt645 不一致')
  })

  it('确认步展示有效/跳过数量，未导入前不出现结果表', async () => {
    const wrapper = mountPage()
    await toConfirmStep(wrapper, [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,1,温控模板,', ',dev_x,TCP-1,modbus-tcp,3,温控模板,'].join('\n'))

    const panel = wrapper.findComponent(ImportResultPanel)
    expect(panel.props('title')).toBe('确认导入以下有效设备')
    expect(panel.props('subTitle')).toBe('共 1 台有效设备，跳过 1 台无效设备')
    expect(wrapper.findAllComponents(DataTable)).toHaveLength(1)
  })

  it('全部成功：逐台创建、结果面板与历史记录都按真实结果记录', async () => {
    const wrapper = mountPage()
    await toConfirmStep(wrapper, [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,1,温控模板,', '电表,dev_meter,TCP-1,modbus-tcp,2,温控模板,'].join('\n'))
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    expect(createDevice).toHaveBeenCalledTimes(2)
    const panel = wrapper.findComponent(ImportResultPanel)
    expect(panel.props('title')).toBe('成功导入 2 台设备，失败 0 台')
    expect(ElMessage.success).toHaveBeenCalledWith('成功导入 2 台设备')

    const outcomes = tableData(wrapper)
    expect(outcomes).toHaveLength(2)
    expect(outcomes.every((o) => o.ok === true)).toBe(true)

    const history = tableData(wrapper, 'last')
    expect(history[0]).toMatchObject({ fileName: 'devices.csv', total: 2, success: 2, failed: 0 })
  })

  it('部分失败：失败设备逐条回填原因，汇总不虚报成功', async () => {
    vi.mocked(createDevice)
      .mockResolvedValueOnce(undefined as never)
      .mockRejectedValueOnce(new Error('设备 dev_meter 已存在'))
    const wrapper = mountPage()
    await toConfirmStep(wrapper, [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,1,温控模板,', '电表,dev_meter,TCP-1,modbus-tcp,2,温控模板,'].join('\n'))
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    expect(wrapper.findComponent(ImportResultPanel).props('title')).toBe('成功导入 1 台设备，失败 1 台')
    expect(ElMessage.warning).toHaveBeenCalledWith('导入完成：成功 1 台，失败 1 台，详见下方结果')
    const outcomes = tableData(wrapper)
    expect(outcomes[1]).toMatchObject({ ok: false, error: '设备 dev_meter 已存在' })

    const history = tableData(wrapper, 'last')
    expect(history[0]).toMatchObject({ total: 2, success: 1, failed: 1 })
  })

  it('下发请求体符合后端契约：Modbus 从站号转数字并补地址，非 Modbus 不带从站号', async () => {
    const wrapper = mountPage([
      { name: '温控模板', protocol: 'modbus-tcp' },
      { name: '电表模板', protocol: 'dlt645' },
    ])
    await toConfirmStep(
      wrapper,
      [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,2,温控模板,', '电表,dev_meter,COM2,dlt645,,电表模板,1234567890'].join('\n'),
    )
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    const payloads = vi.mocked(createDevice).mock.calls.map((c) => c[0]) as Array<Record<string, unknown>>
    // Modbus：从站号是数字，「地址」留空时自动补成从站号；interval 置空交给模板默认周期
    expect(payloads[0]).toMatchObject({ id: 'dev_hvac', slave_id: 2, address: '2', interval: null })
    // 非 Modbus：用表地址寻址，从站号为空
    expect(payloads[1]).toMatchObject({ id: 'dev_meter', slave_id: null, address: '1234567890' })
  })

  it('上一步回到上传步时文件内容仍在，可改文件重试', async () => {
    const wrapper = mountPage()
    await gotoUpload(wrapper)
    await uploadCsv(wrapper, [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,1,温控模板,'].join('\n'))
    await button(wrapper, '下一步').trigger('click')
    await settle()

    await button(wrapper, '上一步').trigger('click')
    await settle()
    expect(wrapper.findComponent(ImportFileUpload).props('rawText')).toContain('dev_hvac')
  })

  it('「开始新的导入」回到下载模板步并清空本轮结果，历史保留', async () => {
    const wrapper = mountPage()
    await toConfirmStep(wrapper, [HEADER, '温控器,dev_hvac,TCP-1,modbus-tcp,1,温控模板,'].join('\n'))
    await button(wrapper, '确认导入').trigger('click')
    await settle()
    expect(wrapper.findAllComponents(DataTable)).toHaveLength(2)

    await button(wrapper, '开始新的导入').trigger('click')
    await settle()

    expect(button(wrapper, '下载 CSV 模板').exists()).toBe(true)
    expect(wrapper.findAllComponents(DataTable)).toHaveLength(1)
    const history = tableData(wrapper)
    expect(history).toHaveLength(1)
  })

  it('没有任何有效设备时点确认导入：不调接口并给出提示', async () => {
    const wrapper = mountPage()
    await toPreviewStep(wrapper, [HEADER, ',dev_x,TCP-1,modbus-tcp,3,温控模板,'].join('\n'))
    expect(previewProps(wrapper).valid).toBe(0)

    await button(wrapper, '下一步').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    expect(ElMessage.warning).toHaveBeenCalledWith('没有可导入的有效设备')
    expect(createDevice).not.toHaveBeenCalled()
  })
})
