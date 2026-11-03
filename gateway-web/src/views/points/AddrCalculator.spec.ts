import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

/**
 * D1：点位管理 › 地址换算（AddrCalculator）行为测试。
 *
 * 这页算出的 0-based 地址和 RTU 请求帧会被现场直接拿去对点。
 * 钉住的核心：
 * - 5 位区地址换算正确（0/1/3/4 四个区、非零偏移）；
 * - 区前缀与功能码不匹配时必须明确指出双方区名，不能算出一个看似合理的地址；
 * - 非法输入/不支持的功能码给出可纠正的提示，不显示半截结果；
 * - 请求帧包含从站号、数量与真实 CRC16，低字节在前。
 */

vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))

const showToast = vi.hoisted(() => vi.fn())

import AddrCalculator from './AddrCalculator.vue'
import ModbusAddressInput from '@/components/industrial/ModbusAddressInput.vue'

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(AddrCalculator)
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

async function setAddr(wrapper: ReturnType<typeof mountPage>, value: string) {
  wrapper.findComponent(ModbusAddressInput).vm.$emit('update:modelValue', value)
  await settle()
}

async function setFuncCode(wrapper: ReturnType<typeof mountPage>, value: string) {
  // 地址输入器内部也有一个区段下拉，功能码下拉是本页最后一个 ElSelect
  wrapper.findAllComponents({ name: 'ElSelect' }).at(-1)!.vm.$emit('update:modelValue', value)
  await settle()
}

async function setNumber(wrapper: ReturnType<typeof mountPage>, which: 'slave' | 'count', value: number) {
  // 地址输入器内部也有一个格式化数字框（max 9999），按 max 区分本页两个输入框
  const target = wrapper
    .findAllComponents({ name: 'ElInputNumber' })
    .find((c) => Number(c.props('max')) === (which === 'slave' ? 247 : 125))
  expect(target, `找不到${which === 'slave' ? '从站 ID' : '数量'}输入框`).toBeTruthy()
  target!.vm.$emit('update:modelValue', value)
  await settle()
}

async function runCalc(wrapper: ReturnType<typeof mountPage>) {
  await button(wrapper, '计算').trigger('click')
  await vi.waitFor(() => expect(wrapper.find('.result').exists()).toBe(true))
  await settle()
}

describe('AddrCalculator 地址换算', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('默认 40001 + FC03 换算为 0-based 0，并给出含 CRC 的 RTU 请求帧', async () => {
    const wrapper = mountPage()
    await runCalc(wrapper)

    expect(wrapper.find('.result').text()).toContain('系统地址(0-based):0')
    expect(wrapper.find('.result').text()).toContain('03 (读保持寄存器)')
    expect(wrapper.find('.hex').text()).toBe('01 03 00 00 00 02 C4 0B')
    expect(wrapper.find('.result').text()).toContain('保持寄存器（4xxxx）')
    expect(wrapper.text()).toContain('仅作调试参考')
  })

  it('四个地址区都能换算：0/1/3/4 前缀各自归零', async () => {
    const cases: Array<[string, string, string]> = [
      ['00001', '01', '读线圈'],
      ['10001', '02', '读离散量输入'],
      ['30001', '04', '读输入寄存器'],
      ['40001', '03', '读保持寄存器'],
    ]
    for (const [addr, fc, name] of cases) {
      const wrapper = mountPage()
      await setAddr(wrapper, addr)
      await setFuncCode(wrapper, fc)
      await runCalc(wrapper)

      expect(wrapper.find('.result').text()).toContain('系统地址(0-based):0')
      expect(wrapper.find('.result').text()).toContain(name)
      wrapper.unmount()
    }
  })

  it('0xxxx 区带前导零的地址必须能算（曾因 parseInt 丢零被判位数不足）', async () => {
    const wrapper = mountPage()
    await setAddr(wrapper, '00001')
    await setFuncCode(wrapper, '01')
    await runCalc(wrapper)

    expect(wrapper.find('.result').text()).toContain('系统地址(0-based):0')
    expect(wrapper.find('.hex').text()).toBe('01 01 00 00 00 02 BD CB')
  })

  it('非零偏移按区基数计算，并从站号/数量正确组帧', async () => {
    const wrapper = mountPage()
    await setAddr(wrapper, '40100')
    await setNumber(wrapper, 'slave', 247)
    await setNumber(wrapper, 'count', 125)
    await runCalc(wrapper)

    expect(wrapper.find('.result').text()).toContain('系统地址(0-based):99')
    expect(wrapper.find('.hex').text()).toBe('F7 03 00 63 00 7D 61 63')
  })

  it('非数字地址：提示可纠正原因，不产出结果', async () => {
    const wrapper = mountPage()
    await setAddr(wrapper, 'abc')
    await button(wrapper, '计算').trigger('click')
    await vi.waitFor(() => expect(showToast).toHaveBeenCalled())

    expect((vi.mocked(showToast).mock.calls[0]![0] as Error).message).toBe(
      '地址格式无效，请输入如 40001 的十进制地址',
    )
    expect(showToast).toHaveBeenCalledWith(expect.any(Error), '计算失败，请检查参数')
    expect(wrapper.find('.result').exists()).toBe(false)
  })

  it('位数/区前缀不合法：明确要求 5 位区地址', async () => {
    const wrapper = mountPage()
    await setAddr(wrapper, '4001')
    await button(wrapper, '计算').trigger('click')
    await vi.waitFor(() => expect(showToast).toHaveBeenCalled())

    expect((vi.mocked(showToast).mock.calls[0]![0] as Error).message).toContain('请输入 5 位区地址')
    expect(wrapper.find('.result').exists()).toBe(false)
  })

  it('功能码与地址区不匹配时点名双方区名，拒绝给出结果', async () => {
    const wrapper = mountPage()
    await setAddr(wrapper, '40001')
    await setFuncCode(wrapper, '01')
    await button(wrapper, '计算').trigger('click')
    await vi.waitFor(() => expect(showToast).toHaveBeenCalled())

    const err = vi.mocked(showToast).mock.calls[0]![0] as Error
    expect(err.message).toContain('功能码 01（读线圈）')
    expect(err.message).toContain('线圈（0xxxx）')
    expect(err.message).toContain('保持寄存器（4xxxx）')
    expect(wrapper.find('.result').exists()).toBe(false)
  })
})
