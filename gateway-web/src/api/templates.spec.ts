import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { deleteTemplate, listTemplates, saveTemplate } from './templates'
import type { DeviceTemplate } from './templates'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('templates 设备模板契约', () => {
  beforeEach(() => requestMock.mockReset())

  // 锁定模板列表路径：GET /api/templates（模板域已从 history 迁到 devices，但路径不变）
  it('listTemplates 走 GET /api/templates 并原样返回列表', async () => {
    const rows: DeviceTemplate[] = [
      { name: '西门子 S7', protocol: 's7', default_interval: 1000, points: [{ name: 'p1', address: 'DB1.DBD0' }] },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listTemplates()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/templates')
  })

  // 锁定模板保存：POST /api/templates，请求体是**原始模板对象**（后端直接吃 Json<DeviceTemplate>），
  // 包成 { template } 会反序列化失败。
  it('saveTemplate 走 POST /api/templates，请求体是原始模板对象', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })
    const tpl: DeviceTemplate = { name: '模板A', protocol: 'modbus', points: [] }

    await saveTemplate(tpl)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/templates')
    expect(cfg.data).toBe(tpl)
  })

  // 锁定模板删除：DELETE /api/templates/:name，名称做 URL 编码
  it('deleteTemplate 走 DELETE /api/templates/:name 并对名称做 URL 编码', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })

    await deleteTemplate('西门子 S7/1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/templates/%E8%A5%BF%E9%97%A8%E5%AD%90%20S7%2F1')
  })

  // 错误分支：后端失败原样 reject，页面据此提示「删除模板失败」
  it('后端报错时 deleteTemplate 原样 reject', async () => {
    requestMock.mockRejectedValueOnce(new Error('请求失败(404): 模板不存在'))
    await expect(deleteTemplate('缺失模板')).rejects.toThrow('模板不存在')
  })
})
