import { describe, it, expect } from 'vitest'
import { isExistingDevice, filterPointsForCopy, mapPointToTarget } from '../utils/deviceCopy'
import type { DeviceConfig, PointConfig } from '@/api/types'

function device(overrides: Partial<DeviceConfig> = {}): DeviceConfig {
  return {
    id: 'd1',
    name: 'dev1',
    template: 'tpl',
    bus: 'bus1',
    protocol: 'modbus',
    address: '1',
    ...overrides,
  } as DeviceConfig
}

function point(overrides: Partial<PointConfig> = {}): PointConfig {
  return {
    id: 'p1',
    bus: 'bus1',
    protocol: 'modbus',
    address: '40001',
    interval: 1,
    scale: 1,
    offset: 0,
    deadband: 0,
    on_change: false,
    ...overrides,
  } as PointConfig
}

describe('isExistingDevice — 设备扫描去重（功能1）', () => {
  const devices = [
    device({ bus: 'bus1', address: '1' }),
    device({ id: 'd2', bus: 'bus1', address: '5' }),
    device({ id: 'd3', bus: 'bus2', address: '1' }),
  ]

  it('同总线同地址 → 已存在', () => {
    expect(isExistingDevice({ address: '1' }, devices, 'bus1')).toBe(true)
  })

  it('同总线不同地址 → 不存在', () => {
    expect(isExistingDevice({ address: '99' }, devices, 'bus1')).toBe(false)
  })

  it('不同总线同地址 → 不存在', () => {
    expect(isExistingDevice({ address: '1' }, devices, 'bus3')).toBe(false)
  })

  it('地址为数字时也能匹配（String 转换）', () => {
    expect(isExistingDevice({ address: 5 }, devices, 'bus1')).toBe(true)
  })

  it('空设备列表 → 不存在', () => {
    expect(isExistingDevice({ address: '1' }, [], 'bus1')).toBe(false)
  })
})

describe('filterPointsForCopy — 设备间点位复制（功能6）', () => {
  const source = [point({ id: 'p1' }), point({ id: 'p2' }), point({ id: 'p3' })]
  const target = [point({ id: 'p2' })]

  it('skip 模式：跳过目标已存在的 id', () => {
    const { toCreate, skipped } = filterPointsForCopy(source, target, 'skip')
    expect(toCreate.map(p => p.id)).toEqual(['p1', 'p3'])
    expect(skipped).toBe(1)
  })

  it('overwrite 模式：全部返回，skipped 为 0', () => {
    const { toCreate, skipped } = filterPointsForCopy(source, target, 'overwrite')
    expect(toCreate).toHaveLength(3)
    expect(skipped).toBe(0)
  })

  it('skip 模式且无冲突时全部返回', () => {
    const { toCreate, skipped } = filterPointsForCopy(source, [], 'skip')
    expect(toCreate).toHaveLength(3)
    expect(skipped).toBe(0)
  })

  it('源为空时返回空', () => {
    const { toCreate, skipped } = filterPointsForCopy([], target, 'skip')
    expect(toCreate).toEqual([])
    expect(skipped).toBe(0)
  })
})

describe('mapPointToTarget — 点位映射到目标设备（功能6）', () => {
  it('设置 device_id 为目标设备 ID', () => {
    const p = point({ id: 'p1', device_id: 'old-device' })
    const mapped = mapPointToTarget(p, 'new-device')
    expect(mapped.device_id).toBe('new-device')
  })

  it('不修改其他字段', () => {
    const p = point({ id: 'p1', name: '温度', address: '40001' })
    const mapped = mapPointToTarget(p, 'dev-target')
    expect(mapped.id).toBe('p1')
    expect(mapped.name).toBe('温度')
    expect(mapped.address).toBe('40001')
  })

  it('原点位不被修改（返回新对象）', () => {
    const p = point({ id: 'p1', device_id: 'old' })
    mapPointToTarget(p, 'new')
    expect(p.device_id).toBe('old')
  })
})
