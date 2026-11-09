import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：设备管理 › 批量导入 · 点位分支（PointImportBranch）行为测试。
 *
 * 这条链路会批量创建真实点位（每条都会落盘并热重载配置），前端校验漏掉一行
 * 就是让脏数据进现场。钉住的核心：
 * - CSV 四步流程能从「下载模板」一路走到「确认导入」（历史上首步没有出路）；
 * - 本地校验结论如实反映（缺字段/表头无数据/JSON 类型错误/部分无效）；
 * - 提交给后端的是「校验通过的 」那批数据，CSV 与 JSON 各自补默认值、保留用户值；
 * - 导入失败要可见报错且不谎报成功；成功后能重置或跳转点位列表。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push: routerPush }) }))
vi.mock('@/utils/exportCsv', () => ({ downloadCsvText: downloadCsvTextMock }))
vi.mock('@/api/points', () => ({ batchImportPoints: batchImportPointsMock }))

const routerPush = vi.hoisted(() => vi.fn())
const downloadCsvTextMock = vi.hoisted(() => vi.fn())
const batchImportPointsMock = vi.hoisted(() => vi.fn())

import PointImportBranch from './PointImportBranch.vue'
import ImportFileUpload from './ImportFileUpload.vue'
import ImportValidatePreview from './ImportValidatePreview.vue'
import PointJsonInputStep from './PointJsonInputStep.vue'
import DataTable from '@/components/DataTable.vue'
import type { PointConfig } from '@/api/types'
import type { PointRow } from './types'

const CSV_HEADER = 'id,name,device_id,bus,protocol,address,slave_id,address_num'
const CSV_OK = [
  CSV_HEADER,
  'p1,温度,dev1,bus1,modbus,40001,1,1',
  'p2,电压,dev2,bus2,dlt645,09000001,,1',
].join('\n')
const CSV_WITH_BAD_ROW = [
  CSV_HEADER,
  'p1,温度,dev1,bus1,modbus,40001,1,1',
  'p_bad,坏行,,,modbus,40002,1,',
].join('\n')
const CSV_HEADER_ONLY = CSV_HEADER
const JSON_PARTIAL = JSON.stringify([
  { id: 'p1', name: '温度', bus: 'bus1', protocol: 'modbus', address: '40001', slave_id: 1, interval: 10, on_change: false },
  { id: 'p2' },
])
const JSON_ALL_INVALID = JSON.stringify([{ id: 'p1' }])

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(PointImportBranch)
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

function previewProps(wrapper: ReturnType<typeof mountPage>) {
  return wrapper.findComponent(ImportValidatePreview).props() as unknown as {
    total: number
    valid: number
    invalid: number
    rows: PointRow[]
  }
}

/** el-radio-group 的 v-model 编译为 modelValue + update:modelValue，直接改模型最稳 */
async function switchMode(wrapper: ReturnType<typeof mountPage>, mode: 'csv' | 'json') {
  wrapper.findComponent({ name: 'ElRadioGroup' }).vm.$emit('update:modelValue', mode)
  await settle()
}

/** 上传 CSV 文本并按「下一步：校验」进入预览步 */
async function csvToPreview(wrapper: ReturnType<typeof mountPage>, csv: string) {
  await button(wrapper, '下一步：上传文件').trigger('click')
  await settle()
  const file = new File([csv], 'points.csv', { type: 'text/csv' })
  wrapper.findComponent(ImportFileUpload).vm.$emit('file-change', { name: 'points.csv', raw: file })
  await vi.waitFor(() =>
    expect(wrapper.findComponent(ImportFileUpload).props('rawText')).toContain('id,name'),
  )
  await settle()
  await button(wrapper, '下一步：校验').trigger('click')
  await settle()
}

/** 粘贴 JSON 文本并按「校验数据」进入预览步 */
async function jsonToPreview(wrapper: ReturnType<typeof mountPage>, text: string) {
  const input = wrapper.findComponent(PointJsonInputStep)
  input.vm.$emit('update:text', text)
  await settle()
  await button(wrapper, '校验数据').trigger('click')
  await settle()
}

describe('PointImportBranch 点位批量导入', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    batchImportPointsMock.mockResolvedValue({ status: 'ok' })
  })

  it('CSV 首步必须给出前进路径：下载模板提示后能进入上传步', async () => {
    const wrapper = mountPage()
    await button(wrapper, '下载 CSV 模板').trigger('click')

    expect(downloadCsvTextMock).toHaveBeenCalledOnce()
    expect(downloadCsvTextMock.mock.calls[0]![1]).toBe('points_template.csv')
    expect(ElMessage.success).toHaveBeenCalledWith('模板已开始下载，填好后点「下一步：上传文件」')

    await button(wrapper, '下一步：上传文件').trigger('click')
    expect(wrapper.findComponent(ImportFileUpload).exists()).toBe(true)
    // 没选文件时不能进入校验
    expect(wrapper.findAll('button').some((b) => b.text().trim() === '下一步：校验')).toBe(false)
  })

  it('CSV 校验：逐行结论如实统计，坏行给出可定位的错误文案', async () => {
    const wrapper = mountPage()
    await csvToPreview(wrapper, CSV_WITH_BAD_ROW)

    const props = previewProps(wrapper)
    expect(props.total).toBe(2)
    expect(props.valid).toBe(1)
    expect(props.invalid).toBe(1)
    expect(props.rows[1]!.error).toContain('缺少 bus')

    const tables = wrapper.findAllComponents(DataTable)
    expect((tables.at(-1)!.props() as unknown as { data: PointRow[] }).data).toHaveLength(2)
  })

  it('CSV 只有表头时拒绝推进，并提示补数据', async () => {
    const wrapper = mountPage()
    await button(wrapper, '下一步：上传文件').trigger('click')
    const file = new File([CSV_HEADER_ONLY], 'points.csv', { type: 'text/csv' })
    wrapper.findComponent(ImportFileUpload).vm.$emit('file-change', { name: 'points.csv', raw: file })
    await vi.waitFor(() =>
      expect(wrapper.findComponent(ImportFileUpload).props('rawText')).toContain('id,name'),
    )
    await settle()
    await button(wrapper, '下一步：校验').trigger('click')

    expect(ElMessage.warning).toHaveBeenCalledWith('文件内容不足，请确保包含表头和至少一行数据')
    expect(wrapper.findComponent(ImportValidatePreview).exists()).toBe(false)
  })

  it('CSV 确认导入：只提交有效行且补齐默认值，成功后给出结果面板', async () => {
    const wrapper = mountPage()
    await csvToPreview(wrapper, CSV_WITH_BAD_ROW)
    await button(wrapper, '下一步：确认导入').trigger('click')
    await settle()

    await button(wrapper, '确认导入').trigger('click')
    await vi.waitFor(() => expect(batchImportPointsMock).toHaveBeenCalledOnce())

    const payload = batchImportPointsMock.mock.calls[0]![0] as PointConfig[]
    expect(payload).toHaveLength(1)
    expect(payload[0]).toMatchObject({ id: 'p1', bus: 'bus1', interval: 1, writable: false })
    expect(ElMessage.success).toHaveBeenCalledWith('成功导入 1 个点位')
    expect(wrapper.text()).toContain('成功导入 1 个点位')
  })

  it('CSV 支持 CT/PT 列：有效值进 payload，非法变比整行判无效', async () => {
    const wrapper = mountPage()
    await csvToPreview(
      wrapper,
      [
        'id,name,device_id,bus,protocol,address,slave_id,address_num,ct_ratio,pt_ratio',
        'p1,电流,dev1,bus1,modbus,40001,1,0,30,',
        'p2,坏变比,dev1,bus1,modbus,40002,1,0,0,',
      ].join('\n'),
    )

    const props = previewProps(wrapper)
    expect(props.valid).toBe(1)
    expect(props.invalid).toBe(1)
    expect(props.rows[1]!.error).toContain('ct_ratio 必须大于 0')

    await button(wrapper, '下一步：确认导入').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await vi.waitFor(() => expect(batchImportPointsMock).toHaveBeenCalledOnce())

    const payload = batchImportPointsMock.mock.calls[0]![0] as PointConfig[]
    expect(payload).toHaveLength(1)
    expect(payload[0]).toMatchObject({ id: 'p1', ct_ratio: 30, pt_ratio: null })
  })

  it('步骤条只允许回跳到已解锁步骤', async () => {
    const wrapper = mountPage()
    await settle()
    // 初始只解锁了第 1 步：点后面的步骤不应切走
    await wrapper.findAll('.el-step')[3]!.trigger('click')
    expect(button(wrapper, '下一步：上传文件').exists()).toBe(true)
    expect(wrapper.findComponent(ImportValidatePreview).exists()).toBe(false)

    await csvToPreview(wrapper, CSV_OK)
    await wrapper.findAll('.el-step')[0]!.trigger('click')
    expect(wrapper.findAll('button').some((b) => b.text().trim() === '下载 CSV 模板')).toBe(true)
  })

  it('切到 JSON 模式：步骤收为 3 步，空文本禁止校验', async () => {
    const wrapper = mountPage()
    await switchMode(wrapper, 'json')

    expect(wrapper.findAll('.el-step__title').map((t) => t.text())).toEqual(['粘贴 JSON', '校验预览', '确认导入'])
    expect(wrapper.findComponent(PointJsonInputStep).exists()).toBe(true)
    expect(button(wrapper, '校验数据').attributes('disabled')).toBeDefined()
  })

  it('JSON 格式错误：提示可定位的文案且不跳步', async () => {
    const wrapper = mountPage()
    await switchMode(wrapper, 'json')
    await jsonToPreview(wrapper, '{bad json')

    expect(ElMessage.warning).toHaveBeenCalledWith('JSON 格式错误，请检查')
    expect(wrapper.findComponent(ImportValidatePreview).exists()).toBe(false)
  })

  it('JSON 部分有效：提示被忽略条数，预览只展示有效行，提交保留用户原值', async () => {
    const wrapper = mountPage()
    await switchMode(wrapper, 'json')
    await jsonToPreview(wrapper, JSON_PARTIAL)

    expect(ElMessage.warning).toHaveBeenCalledWith(expect.stringContaining('1 条点位校验失败（已忽略）'))
    const props = previewProps(wrapper)
    expect(props.total).toBe(1)
    expect(props.valid).toBe(1)
    expect(props.invalid).toBe(0)

    await button(wrapper, '下一步：确认导入').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await vi.waitFor(() => expect(batchImportPointsMock).toHaveBeenCalledOnce())

    const payload = batchImportPointsMock.mock.calls[0]![0] as PointConfig[]
    expect(payload).toHaveLength(1)
    // JSON 路径不补 CSV 默认值，用户的 interval 原样提交
    expect(payload[0]).toMatchObject({ id: 'p1', interval: 10 })
    expect((payload[0] as Record<string, unknown>).writable).toBeUndefined()
  })

  it('JSON 全部无效：提示第一条具体错误，不进入预览', async () => {
    const wrapper = mountPage()
    await switchMode(wrapper, 'json')
    await jsonToPreview(wrapper, JSON_ALL_INVALID)

    expect(ElMessage.warning).toHaveBeenCalledWith('第 1 条 bus 字段必填且必须是非空字符串')
    expect(wrapper.findComponent(ImportValidatePreview).exists()).toBe(false)
  })

  it('导入失败：可见报错且不谎报成功，仍停在确认步可重试', async () => {
    batchImportPointsMock.mockRejectedValueOnce(new Error('500'))
    const wrapper = mountPage()
    await csvToPreview(wrapper, CSV_OK)
    await button(wrapper, '下一步：确认导入').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('导入失败，请检查数据后重试')
    expect(wrapper.text()).not.toContain('成功导入')
    expect(wrapper.findAll('button').some((b) => b.text().trim() === '确认导入')).toBe(true)
  })

  it('成功后：可重新开始（状态清空）或跳转点位列表', async () => {
    const wrapper = mountPage()
    await csvToPreview(wrapper, CSV_OK)
    await button(wrapper, '下一步：确认导入').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await vi.waitFor(() => expect(wrapper.text()).toContain('成功导入 2 个点位'))

    await button(wrapper, '前往点位列表').trigger('click')
    expect(routerPush).toHaveBeenCalledWith('/points')

    await button(wrapper, '开始新的导入').trigger('click')
    expect(button(wrapper, '下载 CSV 模板').exists()).toBe(true)
    expect(wrapper.findComponent(ImportValidatePreview).exists()).toBe(false)
    expect(wrapper.findAll('button').some((b) => b.text().trim() === '确认导入')).toBe(false)
  })
})
