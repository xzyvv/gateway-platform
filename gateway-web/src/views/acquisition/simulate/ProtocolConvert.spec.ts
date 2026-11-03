import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElInput, ElInputNumber, ElRadioGroup } from 'element-plus'

vi.mock('@/api/protocolTools', () => ({
  listConvertRules: vi.fn(),
  createConvertRule: vi.fn(),
  updateConvertRule: vi.fn(),
  deleteConvertRule: vi.fn(),
  toggleConvertRule: vi.fn(),
  PROTO_LABEL: { 'modbus-tcp': 'Modbus TCP', 'modbus-rtu': 'Modbus RTU', mqtt: 'MQTT', 'opc-ua': 'OPC UA' },
}))
vi.mock('@/utils/confirm', () => ({ confirmDelete: vi.fn() }))

import ProtocolConvert from './ProtocolConvert.vue'
import { listConvertRules } from '@/api/protocolTools'

/**
 * 协议转换「本地试跑」：现场靠它确认规则填得对不对，再下发到网关。
 * 试跑公式与后端不一致时，人会在页面上看到“正确”的结果，实际上行却是错的，
 * 所以这里把三种模板的换算口径钉死。
 */

const wrappers: Array<ReturnType<typeof mount>> = []

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  vi.mocked(listConvertRules).mockResolvedValue([])
  const wrapper = mount(ProtocolConvert, { attachTo: document.body })
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

async function openDialog(wrapper: ReturnType<typeof mount>) {
  const btn = wrapper
    .findAll('button')
    .find((b) => b.text().includes('新建转换规则'))
  expect(btn, '找不到「新建转换规则」按钮').toBeTruthy()
  await btn!.trigger('click')
  await settle()
}

/** 弹窗走 teleport，试跑输出只能从 body 读 */
function previewOut(): string {
  return document.body.querySelector('.preview-out')?.textContent?.trim() ?? ''
}

async function setPreviewRaw(wrapper: ReturnType<typeof mount>, value: string) {
  const input = wrapper
    .findAllComponents(ElInput)
    .find((c) => c.props('placeholder') === '输入采集原始值')
  expect(input, '找不到试跑输入框').toBeTruthy()
  input!.vm.$emit('update:modelValue', value)
  await settle()
}

async function setTransform(wrapper: ReturnType<typeof mount>, value: 'raw' | 'linear' | 'enum') {
  wrapper.findComponent(ElRadioGroup).vm.$emit('update:modelValue', value)
  await settle()
}

async function setEnumItem(wrapper: ReturnType<typeof mount>, key: string, label: string) {
  const keyInput = wrapper.findAllComponents(ElInput).find((c) => c.props('placeholder') === '值')
  const labelInput = wrapper.findAllComponents(ElInput).find((c) => c.props('placeholder') === '标签')
  expect(keyInput, '找不到枚举「值」输入框').toBeTruthy()
  expect(labelInput, '找不到枚举「标签」输入框').toBeTruthy()
  keyInput!.vm.$emit('update:modelValue', key)
  labelInput!.vm.$emit('update:modelValue', label)
  await settle()
}

function numberInputs(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAllComponents(ElInputNumber)
}

describe('ProtocolConvert 本地试跑', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
  })

  afterEach(() => {
    for (const w of wrappers) w.unmount()
    wrappers.length = 0
    document.body.innerHTML = ''
  })

  it('原值模板：输出与输入完全一致，不做任何改写', async () => {
    const wrapper = await mountPage()
    await openDialog(wrapper)

    await setPreviewRaw(wrapper, '25')
    expect(previewOut()).toBe('25')
  })

  it('线性模板：按系数/偏移换算，负数与小数不丢精度', async () => {
    const wrapper = await mountPage()
    await openDialog(wrapper)
    await setTransform(wrapper, 'linear')

    // -3.5 * 2 + (-10) = -17；用可精确表示的数避免浮点误差干扰断言
    numberInputs(wrapper)[0]!.vm.$emit('update:modelValue', 2)
    numberInputs(wrapper)[1]!.vm.$emit('update:modelValue', -10)
    await settle()
    await setPreviewRaw(wrapper, '-3.5')

    expect(previewOut()).toBe('-17')
  })

  it('枚举模板：命中枚举值输出标签，未命中给出兜底文案', async () => {
    const wrapper = await mountPage()
    await openDialog(wrapper)
    await setTransform(wrapper, 'enum')

    await setEnumItem(wrapper, '1', '运行')
    await setPreviewRaw(wrapper, '1')
    expect(previewOut()).toBe('运行')

    await setPreviewRaw(wrapper, '9')
    // 不能把未命中的原始值当成转换结果返回，否则现场会按“9”直接上送
    expect(previewOut()).toBe('未匹配任何枚举项')
  })

  it('线性模板输入非数字时给出中文提示，不产生数值结果', async () => {
    const wrapper = await mountPage()
    await openDialog(wrapper)
    await setTransform(wrapper, 'linear')

    await setPreviewRaw(wrapper, 'abc')
    expect(previewOut()).toBe('原始值需为数值')
  })

  it('切换转换方式后重算结果，不残留上一次模板的换算值', async () => {
    const wrapper = await mountPage()
    await openDialog(wrapper)

    await setPreviewRaw(wrapper, '10')
    expect(previewOut()).toBe('10')

    await setTransform(wrapper, 'linear')
    numberInputs(wrapper)[0]!.vm.$emit('update:modelValue', 2)
    await settle()
    expect(previewOut()).toBe('20')

    await setTransform(wrapper, 'enum')
    // 枚举映射仍为空，结果必须是“未匹配”，而不是上一模板的 20
    expect(previewOut()).toBe('未匹配任何枚举项')
    expect(previewOut()).not.toBe('20')
  })
})
