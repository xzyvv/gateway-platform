import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { listDevices, createDevice, updateDevice, deleteDevice, getIoStatus, controlIo } from './devices'
import type { DeviceConfig } from './types'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function deviceRow(patch: Partial<DeviceConfig> = {}): DeviceConfig {
  return {
    id: 'dev-1',
    name: '温度计',
    template: 'tmpl-1',
    bus: 'bus1',
    protocol: 'modbus',
    address: '40001',
    slave_id: 1,
    address_map: null,
    interval: 10,
    ...patch,
  }
}

describe('devices 设备 CRUD 契约', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定列表契约：设备集合走 GET /api/devices（复数），不是 /api/device 也不是 /api/devices/list
  it('设备列表走 GET /api/devices，并原样返回后端数组', async () => {
    const rows = [deviceRow(), deviceRow({ id: 'dev-2', name: '压力计' })]
    requestMock.mockResolvedValue(rows)

    await expect(listDevices()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices')
  })

  // 锁定创建契约：POST 同路径 + 请求体原样透传整个 DeviceConfig（含 id）
  it('创建设备走 POST /api/devices，请求体原样透传设备配置', async () => {
    const payload = deviceRow()
    requestMock.mockResolvedValue(payload)

    await expect(createDevice(payload)).resolves.toEqual(payload)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/devices')
    expect(cfg.data).toEqual(payload)
  })

  // 锁定更新契约：PUT /api/devices/:id（id 进路径，body 里仍带 id）
  it('更新设备走 PUT /api/devices/:id', async () => {
    const payload = deviceRow({ name: '新名字' })
    requestMock.mockResolvedValue(payload)

    await updateDevice('dev-1', payload)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/devices/dev-1')
    expect(cfg.data).toEqual(payload)
  })

  // 锁定删除契约：DELETE /api/devices/:id，与更新共用同一路径前缀
  it('删除设备走 DELETE /api/devices/:id', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })

    await deleteDevice('dev-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/devices/dev-1')
  })

  // 锁定 GPIO 读路径：/api/io/status（与写路径 /api/io/:index 区分）
  it('GPIO 状态走 GET /api/io/status', async () => {
    const io = { di: [true, false], di_quality: [true, true], do_state: [false, false], platform_supported: true }
    requestMock.mockResolvedValue(io)

    await expect(getIoStatus()).resolves.toEqual(io)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/io/status')
  })

  // 锁定 GPIO 写路径：引脚序号进路径，action 进 body（字段名必须是 action）
  it('GPIO 控制走 POST /api/io/:index，请求体为 { action }', async () => {
    requestMock.mockResolvedValue({ status: 'ok', index: 2, on: true })

    await controlIo(2, 'on')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/io/2')
    expect(cfg.data).toEqual({ action: 'on' })
  })

  // 锁定错误分支：后端失败时 promise reject 原样抛出，不在此处吞掉/包装
  it('后端报错时原样 reject（不在 api 层吞掉错误）', async () => {
    requestMock.mockRejectedValue(new Error('设备不存在'))

    await expect(listDevices()).rejects.toThrow('设备不存在')
  })
})
