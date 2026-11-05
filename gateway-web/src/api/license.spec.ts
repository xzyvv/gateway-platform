import { describe, it, expect, vi, beforeEach } from 'vitest'

// license.ts 从 './http' 导入 request（不是 './index'），mock 路径须与被测模块一致
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./http', () => ({ request: requestMock }))

import { fetchLicense, importLicense } from './license'
import type { LicenseSnapshot } from './generated/LicenseSnapshot'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function snapshot(): LicenseSnapshot {
  return {
    machine: {
      machine_key: 'abc123',
      onboard_mac: null,
      storage_serial: null,
      device_serial: null,
    },
    present: true,
    tier: 'standard',
    quota: {
      max_points: 1000,
      max_protocols: null,
      storage_days: 30,
      northbound_channels: null,
    },
    quota_source: 'license',
    status: 'ok',
    expires_at: null,
    license_id: 'lic-1',
    customer: '某公司',
  }
}

describe('license（system 域授权状态契约）', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定查询路径：GET /api/license（无 /import 后缀、无 /state 后缀）
  it('fetchLicense 走 GET /api/license 且不带请求体', async () => {
    requestMock.mockResolvedValue(snapshot())

    await fetchLicense()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/license')
    expect(cfg.data).toBeUndefined()
  })

  // 锁定透传：后端返回裸 LicenseSnapshot（非 ApiResponse 包装），前端不改形
  it('fetchLicense 原样透传后端快照', async () => {
    const snap = snapshot()
    requestMock.mockResolvedValue(snap)

    await expect(fetchLicense()).resolves.toEqual(snap)
  })

  // 锁定导入契约：POST /api/license/import，请求体字段名为 content（.lic 全文）
  it('importLicense 走 POST /api/license/import，请求体为 { content }', async () => {
    requestMock.mockResolvedValue(snapshot())

    await importLicense('LICENSE-CONTENT-ABC')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/license/import')
    expect(cfg.data).toEqual({ content: 'LICENSE-CONTENT-ABC' })
  })

  // 边界：.lic 文件全文是多行文本，必须整体原样透传，不做裁剪/转义
  it('importLicense 原样透传多行 license 文本', async () => {
    requestMock.mockResolvedValue(snapshot())

    const content = 'line1\nline2\nline3'
    await importLicense(content)

    const cfg = firstCallConfig()
    expect((cfg.data as { content: string }).content).toBe(content)
  })

  // 锁定导入后返回最新快照（导入后界面立即刷新状态）
  it('importLicense 成功后返回后端给出的最新快照', async () => {
    const snap = snapshot()
    requestMock.mockResolvedValue(snap)

    await expect(importLicense('x')).resolves.toEqual(snap)
  })

  // 错误分支：校验失败（400 + 中文 message）必须 reject，由对话框展示具体原因
  it('后端校验失败时 importLicense 原样 reject', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(400): license 签名校验失败'))

    await expect(importLicense('bad')).rejects.toThrow('签名校验失败')
  })
})
