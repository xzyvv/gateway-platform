import { describe, it, expect, vi, beforeEach } from 'vitest'

const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { listProtocolCapabilities, getProtocolDiag } from './protocols'

interface RequestConfig {
  url: string
  method: string
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('protocols 能力元数据契约', () => {
  beforeEach(() => requestMock.mockReset())

  // 锁定能力元数据契约：后端真实路径是 GET /api/protocols，
  // 防止路径回退（如误改成 /api/protocol/capabilities）导致写入入口门控静默失效。
  it('能力列表走 GET /api/protocols', async () => {
    requestMock.mockResolvedValue([])
    await listProtocolCapabilities()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/protocols')
  })

  it('原样透传后端响应（name + capabilities）', async () => {
    const resp = [
      {
        name: 'modbus',
        capabilities: {
          supports_batch_read: true,
          supports_frame_extract: true,
          supports_write: true,
          supports_scan_slaves: true,
          supports_register_scan: true,
          supports_auto_detect: true,
        },
      },
      {
        name: 'cjt188',
        capabilities: {
          supports_batch_read: false,
          supports_frame_extract: true,
          supports_write: false,
          supports_scan_slaves: false,
          supports_register_scan: false,
          supports_auto_detect: false,
        },
      },
    ]
    requestMock.mockResolvedValue(resp)

    await expect(listProtocolCapabilities()).resolves.toEqual(resp)
  })

  // 锁定诊断端点契约：路径必须与后端 `public_read` 组注册的
  // `/api/protocols/:name/diag` 一致，避免协议诊断入口静默 404。
  it('协议诊断走 GET /api/protocols/:name/diag', async () => {
    requestMock.mockResolvedValue({ protocol: 's7comm', supported: true, entries: [] })
    await getProtocolDiag('s7comm')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/protocols/s7comm/diag')
  })

  it('协议名做 URL 编码，避免特殊字符拼出非法路径', async () => {
    requestMock.mockResolvedValue({ protocol: 'x', supported: false, entries: [] })
    await getProtocolDiag('a/b')

    expect(firstCallConfig().url).toBe('/api/protocols/a%2Fb/diag')
  })
})
