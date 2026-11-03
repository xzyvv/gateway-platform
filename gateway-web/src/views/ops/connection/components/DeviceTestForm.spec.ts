import { describe, it, expect } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import DeviceTestForm from './DeviceTestForm.vue'
import type { DeviceRow } from './types'

/** 构造一个最小合法的设备行（满足 DeviceConfig 必填字段），可覆盖测试状态字段 */
function makeDevice(over: Partial<DeviceRow> = {}): DeviceRow {
  return {
    id: 'd1',
    name: '电表1',
    template: 'tpl',
    bus: 'modbus-1',
    protocol: 'modbus',
    address: '1',
    slave_id: 1,
    address_map: null,
    interval: 5,
    ...over,
  }
}

/** 等待 el-table 在挂载后完成 body 行渲染（jsdom 下需释放微任务 + 调度） */
async function settle() {
  await flushPromises()
  await nextTick()
}

describe('DeviceTestForm.vue — 设备采集链路测试表单', () => {
  it('渲染表头计数与设备名称行', async () => {
    const devices = [makeDevice({ name: '设备A' }), makeDevice({ id: 'd2', name: '设备B' })]
    const wrapper = mount(DeviceTestForm, { props: { devices, testing: false } })
    await settle()
    const text = wrapper.text()
    expect(text).toContain('设备采集链路（2）')
    expect(text).toContain('设备A')
    expect(text).toContain('设备B')
  })

  it('设备列表为空时"测试全部设备"按钮禁用', () => {
    const wrapper = mount(DeviceTestForm, { props: { devices: [], testing: false } })
    const btn = wrapper.find('.ct-section-header button')
    expect(btn.attributes('disabled')).toBeDefined()
  })

  it('点击"测试全部设备"触发 test-all 事件', async () => {
    const wrapper = mount(DeviceTestForm, {
      props: { devices: [makeDevice()], testing: false },
    })
    const btn = wrapper.find('.ct-section-header button')
    await btn.trigger('click')
    const events = wrapper.emitted('test-all')
    expect(events).toBeTruthy()
    expect(events!.length).toBe(1)
  })

  it('点击单行"测试"按钮触发 test-one 事件并携带该设备行', async () => {
    const dev = makeDevice({ id: 'd1', name: '设备X' })
    const wrapper = mount(DeviceTestForm, {
      props: { devices: [dev], testing: false },
    })
    await settle()
    const rowBtn = wrapper.findAll('button').find((b) => b.text().trim() === '测试')
    expect(rowBtn).toBeTruthy()
    await rowBtn!.trigger('click')
    const events = wrapper.emitted('test-one')!
    expect(events.length).toBe(1)
    expect(events[0]![0]).toStrictEqual(dev)
  })

  it('根据 _status 渲染对应状态标签文案与错误详情', async () => {
    const devices = [
      makeDevice({ id: 'd1', name: '设备1', _status: 'ok' }),
      makeDevice({ id: 'd2', name: '设备2', _status: 'fail', _error: '读取超时' }),
      makeDevice({ id: 'd3', name: '设备3', _status: 'testing' }),
      makeDevice({ id: 'd4', name: '设备4', _status: 'idle' }),
    ]
    const wrapper = mount(DeviceTestForm, { props: { devices, testing: false } })
    await settle()
    const text = wrapper.text()
    expect(text).toContain('在线')
    expect(text).toContain('离线')
    expect(text).toContain('测试中')
    expect(text).toContain('未测试')
    expect(text).toContain('读取超时')
    expect(text).toContain('读取成功')
  })

  it('testing=true 时"测试全部设备"按钮处于 loading 态', () => {
    const wrapper = mount(DeviceTestForm, {
      props: { devices: [makeDevice()], testing: true },
    })
    const btn = wrapper.find('.ct-section-header button')
    expect(btn.classes().some((c) => c.includes('loading'))).toBe(true)
  })
})