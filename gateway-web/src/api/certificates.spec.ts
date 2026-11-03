import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { listCertificates, renewCertificate, uploadCertificate } from './certificates'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('certificates 证书管理契约', () => {
  beforeEach(() => requestMock.mockReset())

  // 锁定列表路径：GET /api/certificates
  it('listCertificates 走 GET /api/certificates 并原样返回列表', async () => {
    const rows = [
      {
        id: 'gateway',
        name: '网关 TLS 证书',
        cert_type: 'server',
        subject: 'CN=gateway',
        issuer: 'CN=gateway',
        serial_number: '01',
        valid_from: 1700000000,
        valid_to: 1800000000,
        days_remaining: 86,
        cert_path: '/etc/gateway.crt',
        fingerprint: 'AA:BB',
        enabled: true,
      },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listCertificates()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/certificates')
  })

  // 锁定上传请求体：字段名是 snake_case（后端 UploadCertRequest: name / cert_pem / key_pem），
  // 误写成 camelCase 会 serde 解析不到 → 证书写空文件。
  it('uploadCertificate 走 POST /api/certificates/upload，请求体为 { name, cert_pem, key_pem }', async () => {
    requestMock.mockResolvedValue(undefined)

    await uploadCertificate('gateway', 'CERT_PEM', 'KEY_PEM')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/certificates/upload')
    expect(cfg.data).toEqual({ name: 'gateway', cert_pem: 'CERT_PEM', key_pem: 'KEY_PEM' })
  })

  // 锁定续期路径：POST /api/certificates/renew（无请求体）
  it('renewCertificate 走 POST /api/certificates/renew 且不带请求体', async () => {
    requestMock.mockResolvedValue(undefined)

    await renewCertificate()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/certificates/renew')
    expect(cfg.data).toBeUndefined()
  })

  // 第 1 期冗余收口：`GET /api/cert/status` 已删除（与列表同源），
  // 其字段 days_remaining / cert_path 并入列表项，且不得再请求旧端点。
  it('证书状态不再请求 /api/cert/status，字段随列表项返回', async () => {
    requestMock.mockResolvedValue([])

    await listCertificates()

    const cfg = firstCallConfig()
    expect(cfg.url).toBe('/api/certificates')
    expect(cfg.url).not.toBe('/api/cert/status')
  })

  // 错误分支：后端失败原样 reject（证书上传失败必须让用户看到原因）
  it('后端报错时 uploadCertificate 原样 reject', async () => {
    requestMock.mockRejectedValueOnce(new Error('请求失败(500): write cert failed'))
    await expect(uploadCertificate('gateway', 'C', 'K')).rejects.toThrow('write cert failed')
  })
})
