import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  listServices,
  listHealEvents,
  restartService,
  ACTION_LABEL,
  STATUS_LABEL,
  STATUS_TYPE,
  type ServiceInfo,
  type HealEvent,
} from './watchdog'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('watchdog（system 域看门狗 / 自愈契约）', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定服务列表路径：GET /api/watchdog/services（不是 /api/system/services）
  it('listServices 走 GET /api/watchdog/services 并原样返回列表', async () => {
    const rows: ServiceInfo[] = [
      {
        name: 'mqtt',
        status: 'Running',
        pid: 100,
        uptime: 3600,
        cpu: 1.5,
        mem: 2048,
        restarts: 0,
      },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listServices()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/watchdog/services')
  })

  // 锁定事件列表路径：GET /api/watchdog/events（与 services 是两个端点，不可混写）
  it('listHealEvents 走 GET /api/watchdog/events 并原样返回列表', async () => {
    const rows: HealEvent[] = [
      {
        id: 'h1',
        time: 1700000000000,
        action: 'RestartService',
        target: 'mqtt',
        reason: '连接超时',
        result: 'success',
      },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listHealEvents()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/watchdog/events')
  })

  // 锁定重启路径：POST /api/watchdog/services/:name/restart（后端为 POST，非 PUT）
  it('restartService 走 POST /api/watchdog/services/:name/restart', async () => {
    requestMock.mockResolvedValue(undefined)

    await restartService('mqtt')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/watchdog/services/mqtt/restart')
    expect(cfg.data).toBeUndefined()
  })

  // 边界：服务名含特殊字符（空格 / 斜杠）时必须做 URL 编码，
  // 否则路径会被拆成多段打到错误路由
  it('restartService 对服务名做 URL 编码', async () => {
    requestMock.mockResolvedValue(undefined)

    await restartService('net manager/1')

    const cfg = firstCallConfig()
    expect(cfg.url).toBe('/api/watchdog/services/net%20manager%2F1/restart')
  })

  // 锁定标签映射：UI 直接用这三个常量渲染，
  // 键缺失会让页面显示 undefined
  it('ACTION_LABEL / STATUS_LABEL / STATUS_TYPE 覆盖全部枚举变体', () => {
    expect(Object.keys(ACTION_LABEL).sort()).toEqual(['ResetNetwork', 'ResetSerial', 'RestartService'])
    expect(Object.keys(STATUS_LABEL).sort()).toEqual(['Restarting', 'Running', 'Stopped', 'Unknown'])
    expect(Object.keys(STATUS_TYPE).sort()).toEqual(['Restarting', 'Running', 'Stopped', 'Unknown'])
    expect(ACTION_LABEL.RestartService).toBe('重启服务')
    expect(STATUS_LABEL.Running).toBe('运行中')
    expect(STATUS_TYPE.Running).toBe('success')
    expect(STATUS_TYPE.Stopped).toBe('danger')
  })

  // 错误分支：服务不存在 / 重启失败时原样 reject，由页面提示
  it('后端报错时 restartService 原样 reject', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(500): 重启服务失败'))

    await expect(restartService('mqtt')).rejects.toThrow('重启服务失败')
  })
})
