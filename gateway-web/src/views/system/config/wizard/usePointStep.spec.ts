import { describe, it, expect, vi, beforeEach } from 'vitest'
import { ElMessage } from 'element-plus'
import { createPoint, listPoints } from '@/api/acquisition/points'
import { usePointStep } from './usePointStep'

vi.mock('@/api/acquisition/points', () => ({
  createPoint: vi.fn(() => Promise.resolve({})),
  listPoints: vi.fn(() => Promise.resolve([])),
}))

vi.mock('element-plus', () => ({
  ElMessage: { warning: vi.fn(), success: vi.fn(), error: vi.fn() },
}))

/**
 * 向导补录点位的回归：地址必须由 5 位串换算出 address_num + function_code。
 *
 * 曾经的实现把 `address_num` 硬编码成 1、`function_code` 硬编码成 3，
 * 用户填什么地址都读 1 号寄存器 —— 点位「有数但全错」，现场极难排查。
 */
describe('usePointStep.addPoint', () => {
  const bus = { port_name: 'COM1', protocol: 'modbus_rtu' }
  const device = { id: 'dev1' }

  beforeEach(() => {
    vi.mocked(createPoint).mockClear()
    vi.mocked(ElMessage.warning).mockClear()
  })

  function payloadOf(): Record<string, unknown> {
    expect(createPoint).toHaveBeenCalledTimes(1)
    return vi.mocked(createPoint).mock.calls[0]![1] as Record<string, unknown>
  }

  it('40001 换算成输入寄存器偏移 0', async () => {
    const { manual, addPoint } = usePointStep()
    manual.value = { name: '电压', address: '40001', dataType: 'f32' }
    await addPoint(device, bus, 1)

    const p = payloadOf()
    expect(p.address).toBe('40001')
    expect(p.address_num).toBe(0)
    expect(p.function_code).toBe(4)
    expect(p.id).toBe('dev1_40001')
  })

  it('保持寄存器 30002 换算成偏移 1', async () => {
    const { manual, addPoint } = usePointStep()
    manual.value = { name: '电流', address: '30002', dataType: 'u16' }
    await addPoint(device, bus, 1)

    const p = payloadOf()
    expect(p.address_num).toBe(1)
    expect(p.function_code).toBe(3)
  })

  it('地址非法时不写接口，只提示', async () => {
    const { manual, addPoint } = usePointStep()
    manual.value = { name: '电压', address: 'abc', dataType: 'f32' }
    await addPoint(device, bus, 1)

    expect(createPoint).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalled()
  })

  it('非 Modbus 总线不写接口（表单按数据标识寻址，不能套寄存器模型）', async () => {
    const { manual, addPoint } = usePointStep()
    manual.value = { name: '电量', address: '40001', dataType: 'u32' }
    await addPoint(device, { port_name: 'COM2', protocol: 'dlt645' }, 1)

    expect(createPoint).not.toHaveBeenCalled()
    expect(listPoints).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalled()
  })
})
