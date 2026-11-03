import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElDrawer, ElSelect } from 'element-plus'

vi.mock('@/api/config', () => ({
  getConfig: vi.fn(),
}))

import ModbusAddressMap from './ModbusAddressMap.vue'
import { getConfig } from '@/api/config'
import type { PointConfig } from '@/api/types'

/**
 * Modbus 地址占用图：
 * 现场拿它判断「某个寄存器地址是否已经被点位占用」，图上一旦标错，
 * 运维就会把已有点位当成空闲地址再建一个，造成地址冲突。
 * 这里钉住渲染、详情、筛选重拉与失败时的真实表现。
 */

const wrappers: Array<ReturnType<typeof mount>> = []

async function settle() {
  await flushPromises()
  await nextTick()
}

/** 两条总线：bus-1 下有从站 1 / 2，bus-2 只有从站 2，便于验证筛选条件 */
function pointsFixture(): PointConfig[] {
  return [
    { id: 'p-1', name: '温度A', bus: 'bus-1', slave_id: 1, address: '10', address_num: 2 },
    { id: 'p-2', name: '压力B', bus: 'bus-1', slave_id: 2, address: '20', address_num: 1 },
    { id: 'p-3', name: '流量C', bus: 'bus-2', slave_id: 2, address: '30', address_num: 1 },
  ] as unknown as PointConfig[]
}

async function mountPage(points: PointConfig[] = pointsFixture()) {
  vi.mocked(getConfig).mockResolvedValue({ points } as never)
  const wrapper = mount(ModbusAddressMap)
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

/** 按 aria-label 定位下拉框（三个下拉的文案在模板里是稳定契约） */
function selectByLabel(wrapper: ReturnType<typeof mount>, label: string) {
  const hit = wrapper
    .findAllComponents(ElSelect)
    .find((c) => c.props('ariaLabel') === label)
  expect(hit, `找不到下拉框「${label}」`).toBeTruthy()
  return hit!
}

async function changeSelect(wrapper: ReturnType<typeof mount>, label: string, value: string | number) {
  const select = selectByLabel(wrapper, label)
  select.vm.$emit('update:modelValue', value)
  select.vm.$emit('change', value)
  await settle()
}

describe('ModbusAddressMap 地址占用图', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    localStorage.clear()
  })

  afterEach(() => {
    for (const w of wrappers) w.unmount()
    wrappers.length = 0
  })

  it('按总线/从站筛选点位，渲染已占用地址与点位名', async () => {
    const wrapper = await mountPage()

    // 默认 100 格；p-1 占 10/11（address_num=2），p-2 占 20
    expect(wrapper.findAll('.address-cell')).toHaveLength(100)
    expect(wrapper.findAll('.address-cell.used')).toHaveLength(3)
    const labels = wrapper.findAll('.address-cell.used .addr-label').map((n) => n.text())
    expect(labels).toEqual(['温度A', '温度A', '压力B'])
    expect(wrapper.text()).toContain('已占用')
  })

  it('点击已占用格子打开占用详情，能看到点位名/ID/寄存器数', async () => {
    const wrapper = await mountPage()

    await wrapper.findAll('.address-cell.used')[0]!.trigger('click')
    await settle()

    // 抽屉标题在关闭时也存在于 DOM，必须断言真正的打开状态
    expect(wrapper.findComponent(ElDrawer).props('modelValue')).toBe(true)
    // el-drawer 默认不 append-to-body：内容可能在 wrapper 内，兜底再读 teleport 后的 body
    const text = `${wrapper.text()}${document.body.textContent ?? ''}`
    expect(text).toContain('地址占用详情')
    expect(text).toContain('温度A')
    expect(text).toContain('p-1')
    expect(text).toContain('占用寄存器数')
    // 地址 10 的寄存器副本应显示 2 格
    expect(text).toContain('2')
  })

  it('点击空闲格子不弹详情，避免把空闲地址误当成占用', async () => {
    const wrapper = await mountPage()

    await wrapper.findAll('.address-cell.free')[0]!.trigger('click')
    await settle()

    // 点击空闲地址不能打开详情：否则会让人以为该地址已被点位占用
    expect(wrapper.findComponent(ElDrawer).props('modelValue')).toBe(false)
  })

  it('无点位时给空态，不画满屏空闲，避免把「没数据」看成「全部空闲」', async () => {
    const wrapper = await mountPage([])

    // 防现场事故：配置为空和「核对过确实没有点位」在满屏空闲的图上无法区分，
    // 现场会照着空位建点；必须用空态明确告知先去配置，而不是渲染 100 格空闲
    expect(wrapper.findAll('.address-cell')).toHaveLength(0)
    expect(wrapper.find('.map-container').exists()).toBe(false)
    expect(wrapper.text()).toContain('暂无点位数据')
    expect(wrapper.text()).toContain('请先在配置管理中添加设备与点位')
  })

  it('配置接口失败时显式报错，不退化成「全部空闲」', async () => {
    vi.mocked(getConfig).mockRejectedValue(new Error('config 接口 500'))
    const wrapper = mount(ModbusAddressMap)
    wrappers.push(wrapper)
    await settle()

    // 防现场事故：读配置失败 ≠ 地址全部空闲。必须把失败原因显示出来，
    // 否则现场照着满屏「空闲」继续建点，点位会落到已被占用的地址上
    expect(wrapper.text()).toContain('配置读取失败')
    expect(wrapper.text()).toContain('config 接口 500')
    expect(wrapper.findAll('.address-cell.used')).toHaveLength(0)
    // 失败优先展示错误原因，不能同时冒出「暂无点位」的空态把失败说成配置为空
    expect(wrapper.text()).not.toContain('暂无点位数据')
  })

  it('切换总线会重新拉取配置，并按新总线过滤占用格', async () => {
    const wrapper = await mountPage()
    expect(vi.mocked(getConfig)).toHaveBeenCalledTimes(1)

    await changeSelect(wrapper, '选择总线', 'bus-2')

    // 防御：换了总线却沿用旧点位，会让人在错误的总线上建点
    expect(vi.mocked(getConfig)).toHaveBeenCalledTimes(2)
    expect(wrapper.findAll('.address-cell.used')).toHaveLength(1)
    expect(wrapper.findAll('.address-cell.used .addr-label')[0]!.text()).toBe('流量C')
    expect(wrapper.text()).not.toContain('温度A')
  })

  it('切换从站会重新拉取配置，只显示该从站的地址占用', async () => {
    const wrapper = await mountPage()

    await changeSelect(wrapper, '选择从站', 2)

    expect(vi.mocked(getConfig)).toHaveBeenCalledTimes(2)
    expect(wrapper.findAll('.address-cell.used')).toHaveLength(1)
    expect(wrapper.findAll('.address-cell.used .addr-label')[0]!.text()).toBe('压力B')
    expect(wrapper.text()).not.toContain('温度A')
  })

  it('切换寄存器类型按功能码分区显示，且不重复请求配置', async () => {
    // 保持寄存器(3) 与输入寄存器(4) 是两套编址：地址 10 与 15 各自独立，
    // 混在一张图上会让人误判「D10 已被输入寄存器占用」
    const wrapper = await mountPage([
      { id: 'p-h', name: '保持A', bus: 'bus-1', slave_id: 1, address: '10', function_code: 3 },
      { id: 'p-i', name: '输入A', bus: 'bus-1', slave_id: 1, address: '15', function_code: 4 },
    ] as unknown as PointConfig[])

    const holdingLabels = wrapper.findAll('.address-cell.used .addr-label').map((n) => n.text())
    expect(holdingLabels).toEqual(['保持A'])

    await changeSelect(wrapper, '寄存器类型', 'input')

    expect(wrapper.findAll('.address-cell.used .addr-label').map((n) => n.text())).toEqual(['输入A'])
    // 纯前端分区过滤，不应再打一次配置接口
    expect(vi.mocked(getConfig)).toHaveBeenCalledTimes(1)
  })
})
