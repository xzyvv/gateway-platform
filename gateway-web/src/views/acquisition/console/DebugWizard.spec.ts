import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElCheckbox, ElRadioGroup, ElSelect } from 'element-plus'

/**
 * D1：现场调试向导行为测试。
 *
 * 这一页最容易误导现场的地方是「自动检查」：结论说错了，人就会往错的方向拆机柜。
 * 所以钉住四件事：
 *   1. 进度计数与勾选状态必须真实反映当前流程，切流程要清空（不能串症状）；
 *   2. 总线/设备自动检查在「没配 / 没选中 / 协议不匹配」三种情况下都要给出可执行入口；
 *   3. 健康检查按协议过滤，MQTT 流程里不该冒出总线类异常；
 *   4. 健康报告读不到时必须报 danger，不能静默说「未发现异常」。
 */

const { tgt, health, push } = vi.hoisted(() => ({
  tgt: {
    buses: [] as Array<Record<string, unknown>>,
    devices: [] as Array<Record<string, unknown>>,
    selectedBus: '',
    selectedDeviceId: '',
    reload: vi.fn(async () => undefined),
  },
  health: {
    report: { problems: [] as Array<Record<string, unknown>> } as Record<string, unknown> | null,
    error: '' as string | null,
    refresh: vi.fn(async () => null as Record<string, unknown> | null),
  },
  push: vi.fn(),
}))

vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))

vi.mock('@/composables/acquisition/useDebugTargets', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  return {
    useDebugTargets: () => ({
      buses: ref(tgt.buses),
      devices: ref(tgt.devices),
      selectedBus: ref(tgt.selectedBus),
      selectedDeviceId: ref(tgt.selectedDeviceId),
      reload: tgt.reload,
    }),
  }
})

vi.mock('@/composables/system/useHealthReport', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  return {
    useHealthReport: () => ({
      report: ref(health.report),
      error: ref(health.error),
      refresh: health.refresh,
    }),
  }
})

import DebugWizard from './DebugWizard.vue'

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(DebugWizard)
  await settle()
  return wrapper
}

/** 当前流程里带「自动检查」按钮的步骤容器 */
function probeSteps(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAll('.step').filter((s) => s.findAll('button').some((b) => b.text() === '自动检查'))
}

async function runProbe(wrapper: ReturnType<typeof mount>, index = 0) {
  const step = probeSteps(wrapper)[index]
  expect(step, '当前流程没有可自动检查的步骤').toBeTruthy()
  const btn = step!.findAll('button').find((b) => b.text() === '自动检查')!
  await btn.trigger('click')
  await settle()
  return step!
}

function probeText(step: ReturnType<typeof probeSteps>[number]): string {
  return step.find('.probe').text()
}

describe('DebugWizard 现场调试向导', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    tgt.buses = []
    tgt.devices = []
    tgt.selectedBus = ''
    tgt.selectedDeviceId = ''
    health.report = { problems: [] }
    health.error = null
    health.refresh.mockResolvedValue({ problems: [] })
  })

  it('默认展示第一条排查路径并给出步骤数、初始进度与汇总卡', async () => {
    const wrapper = await mountPage()
    expect(wrapper.text()).toContain('读不到数 / 从站无应答')
    expect(wrapper.text()).toContain('0/5 已完成')
    expect(wrapper.findAll('.step')).toHaveLength(5)
    // 汇总卡是交接班的唯一结论来源：初始就必须说清「还差多少」
    const summary = wrapper.find('.summary')
    expect(summary.text()).toContain('排查汇总')
    expect(summary.text()).toContain('还有 5 项检查未完成')
    expect(summary.find('.next').exists()).toBe(true)
  })

  it('勾选检查项后进度增加且该步骤标记为已完成', async () => {
    const wrapper = await mountPage()
    const boxes = wrapper.findAllComponents(ElCheckbox)
    boxes[0]!.vm.$emit('update:modelValue', true)
    await settle()

    expect(wrapper.text()).toContain('1/5 已完成')
    expect(wrapper.findAll('.step')[0]!.classes()).toContain('done')
  })

  it('切换症状会清空勾选与上次的自动检查结论，避免结论串到另一条路径', async () => {
    const wrapper = await mountPage()
    const boxes = wrapper.findAllComponents(ElCheckbox)
    boxes[0]!.vm.$emit('update:modelValue', true)
    await settle()
    await runProbe(wrapper)
    expect(wrapper.text()).toContain('1/5 已完成')

    wrapper.findComponent(ElSelect).vm.$emit('update:modelValue', 'modbus-rtu-crc-fail')
    await settle()

    expect(wrapper.find('.progress').text()).toMatch(/^0\//)
    expect(probeSteps(wrapper)[0]!.find('.probe').exists()).toBe(false)
  })

  it('切换协议筛选后，若当前症状不属于该协议则自动落到该协议第一条', async () => {
    const wrapper = await mountPage()
    wrapper.findComponent(ElRadioGroup).vm.$emit('update:modelValue', 'mqtt')
    await settle()

    expect(wrapper.text()).toContain('MQTT 上行')
    // 症状下拉只剩 mqtt 路径，当前流程必须被同步切过去
    expect(wrapper.text()).not.toContain('读不到数 / 从站无应答')
  })

  it('总线检查：未配置任何总线时给出「去配置管理」入口', async () => {
    const wrapper = await mountPage()
    const step = await runProbe(wrapper)

    expect(probeText(step)).toContain('未配置总线')
    expect(step.classes()).not.toContain('done')
    const goto = step.findAll('button').find((b) => b.text() === '去配置管理')
    expect(goto).toBeTruthy()
    await goto!.trigger('click')
    expect(push).toHaveBeenCalledWith('/system/config?tab=wizard')
  })

  it('总线检查：已配置但未选中时提示先选总线', async () => {
    tgt.buses = [{ port_name: 'bus-1', protocol: 'modbus' }]
    const wrapper = await mountPage()
    const step = await runProbe(wrapper)

    expect(probeText(step)).toContain('尚未选中调试总线')
    expect(step.find('.probe').classes()).toContain('warning')
  })

  it('总线检查：协议不匹配时点名两边的协议，不给出「匹配」的假结论', async () => {
    tgt.buses = [{ port_name: 'bus-1', protocol: 'modbus', tcp_addr: '192.168.1.10:502' }]
    tgt.selectedBus = 'bus-1'
    const wrapper = await mountPage()
    const step = await runProbe(wrapper)

    const text = probeText(step)
    expect(text).toContain('bus-1')
    // 提示里用展示名「Modbus TCP」，与筛选按钮同一套文案
    expect(text).toContain('Modbus TCP')
    expect(text).toContain('Modbus RTU')
    expect(text).toContain('不匹配')
    expect(step.find('.probe').classes()).toContain('warning')
  })

  it('总线检查：协议匹配时给出成功结论', async () => {
    tgt.buses = [{ port_name: 'bus-1', protocol: 'modbus' }]
    tgt.selectedBus = 'bus-1'
    const wrapper = await mountPage()
    const step = await runProbe(wrapper)

    expect(probeText(step)).toContain('与本次排查协议匹配')
    expect(step.find('.probe').classes()).toContain('success')
  })

  it('设备检查：有设备但未选中时提示先选设备', async () => {
    tgt.buses = [{ port_name: 'bus-1', protocol: 'modbus' }]
    tgt.devices = [{ id: 'dev-1', name: '电表一号' }, { id: 'dev-2', name: '压力表' }]
    const wrapper = await mountPage()
    wrapper.findComponent(ElRadioGroup).vm.$emit('update:modelValue', 'generic')
    await settle()
    const step = await runProbe(wrapper)

    expect(probeText(step)).toContain('2 台设备')
    expect(probeText(step)).toContain('未选中')
  })

  it('设备检查：选中设备后给出设备名与 ID', async () => {
    tgt.devices = [{ id: 'dev-1', name: '电表一号' }]
    tgt.selectedDeviceId = 'dev-1'
    const wrapper = await mountPage()
    wrapper.findComponent(ElRadioGroup).vm.$emit('update:modelValue', 'generic')
    await settle()
    const step = await runProbe(wrapper)

    expect(probeText(step)).toContain('电表一号')
    expect(probeText(step)).toContain('dev-1')
  })

  it('健康检查：报告读取失败时报 danger，而不是「未发现异常」', async () => {
    health.refresh.mockResolvedValue(null)
    health.error = 'health 接口 500'
    const wrapper = await mountPage()
    // 第一条路径的最后一个步骤是健康检查
    const step = await runProbe(wrapper, 1)

    expect(probeText(step)).toContain('健康报告读取失败')
    expect(step.find('.probe').classes()).toContain('danger')
  })

  it('健康检查：相关异常按条列出并带上机器码', async () => {
    health.refresh.mockResolvedValue({
      problems: [
        { level: 'critical', code: 'BUS_ALL_OFFLINE', message: '总线 bus-1 设备全部离线', hint: '查接线' },
        { level: 'warning', code: 'TIMEOUT_HIGH', message: '采集超时率偏高', hint: '查参数' },
      ],
    })
    const wrapper = await mountPage()
    const step = await runProbe(wrapper, 1)

    expect(probeText(step)).toContain('发现 2 条与本次排查相关的异常')
    expect(probeText(step)).toContain('BUS_ALL_OFFLINE')
    expect(step.find('.probe').classes()).toContain('warning')
  })

  it('MQTT 流程里过滤掉总线类异常，只保留上行相关问题', async () => {
    health.refresh.mockResolvedValue({
      problems: [
        { level: 'critical', code: 'BUS_ALL_OFFLINE', message: '总线 bus-1 设备全部离线', hint: '查接线' },
        { level: 'critical', code: 'MQTT_DISCONNECTED', message: 'MQTT 已断开', hint: '查 broker' },
        { level: 'warning', code: 'QUEUE_BACKPRESSURE', message: '待发送队列积压', hint: '查上行' },
      ],
    })
    const wrapper = await mountPage()
    wrapper.findComponent(ElRadioGroup).vm.$emit('update:modelValue', 'mqtt')
    await settle()
    const step = await runProbe(wrapper)

    const text = probeText(step)
    expect(text).toContain('发现 2 条与本次排查相关的异常')
    expect(text).toContain('MQTT_DISCONNECTED')
    expect(text).not.toContain('BUS_ALL_OFFLINE')
  })

  it('自动检查抛异常时给出 danger 结论而不弹出页面级错误', async () => {
    tgt.reload.mockRejectedValueOnce(new Error('配置接口不可用'))
    const wrapper = await mountPage()
    const step = await runProbe(wrapper)

    expect(probeText(step)).toContain('配置接口不可用')
    expect(step.find('.probe').classes()).toContain('danger')
  })

  it('全部勾选但自动检查未执行时，汇总卡不给出「未发现异常」的假结论', async () => {
    const wrapper = await mountPage()
    // 逐项确认：全部打完勾也不能漏计（漏计会让现场以为还有检查项没做）
    for (const box of wrapper.findAllComponents(ElCheckbox)) {
      box.vm.$emit('update:modelValue', true)
    }
    await settle()

    expect(wrapper.find('.progress').text()).toBe('5/5 已完成')
    expect(wrapper.findAll('.step.done')).toHaveLength(5)
    // 勾选全完成 ≠ 自动检查全跑过：必须如实告知还有 2 项没执行
    const summary = wrapper.find('.summary')
    expect(summary.text()).toContain('检查项已全部确认，但还有 2 项自动检查未执行')
    expect(summary.classes()).not.toContain('success')
  })

  it('勾选全部检查项且自动检查全通过时，汇总卡给出通过结论并收起下一步', async () => {
    tgt.buses = [{ port_name: 'bus-1', protocol: 'modbus' }]
    tgt.selectedBus = 'bus-1'
    const wrapper = await mountPage()
    // 两条自动检查：总线协议匹配 + 健康报告无相关异常
    await runProbe(wrapper, 0)
    await runProbe(wrapper, 1)
    for (const box of wrapper.findAllComponents(ElCheckbox)) {
      box.vm.$emit('update:modelValue', true)
    }
    await settle()

    const summary = wrapper.find('.summary')
    expect(summary.classes()).toContain('success')
    expect(summary.text()).toContain('全部检查项已完成，自动检查未发现异常')
    // 全部通过后不再甩「下一步」按钮，避免把已完成的路径又推回去
    expect(summary.find('.next').exists()).toBe(false)
  })

  it('自动检查报严重问题时，汇总卡立刻变红并点名需处理数量', async () => {
    tgt.reload.mockRejectedValueOnce(new Error('配置接口不可用'))
    const wrapper = await mountPage()
    await runProbe(wrapper, 0)

    const summary = wrapper.find('.summary')
    expect(summary.classes()).toContain('danger')
    expect(summary.text()).toContain('1 项严重问题')
    expect(summary.text()).toContain('自动检查 1/2 项已执行')
    // 「进度 0/5」时也必须先把严重问题顶到最前，不能被进度掩盖
    expect(summary.text()).toContain('请先按步骤内的提示处置')
  })

  it('切换症状后汇总卡清空上次结论，不会把上一路径的进度带过来', async () => {
    tgt.reload.mockRejectedValueOnce(new Error('配置接口不可用'))
    const wrapper = await mountPage()
    await runProbe(wrapper, 0)
    expect(wrapper.find('.summary').classes()).toContain('danger')

    wrapper.findComponent(ElSelect).vm.$emit('update:modelValue', 'modbus-rtu-crc-fail')
    await settle()

    const summary = wrapper.find('.summary').text()
    expect(summary).not.toContain('严重问题')
    expect(summary).toContain('自动检查 0/')
  })

  it('「去处理」按钮跳转到对应页签', async () => {
    const wrapper = await mountPage()
    const step = wrapper.findAll('.step')[1]!
    const btn = step.findAll('button').find((b) => b.text() === '去处理 · 报文捕获看原始帧')
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(push).toHaveBeenCalledWith('/acquisition/console?tab=packet-capture')
  })
})
