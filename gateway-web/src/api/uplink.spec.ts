import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  getUplinkConfig,
  saveUplinkConfig,
  listTemplateVars,
  testUplink,
  exportTemplate,
  importTemplate,
  listTestHistory,
  clearTestHistory,
  getPassthroughSessions,
  getNorthStatus,
  BUILTIN_TEMPLATES,
  type UplinkConfig,
} from './uplink'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function uplinkCfg(patch: Partial<UplinkConfig> = {}): UplinkConfig {
  return {
    enabled: true,
    template: '{"v":${value}}',
    encoding: 'json',
    max_bytes: 4096,
    updated_at: '2026-01-01T00:00:00Z',
    ...patch,
  }
}

describe('uplink 模板配置契约', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定读取契约：GET /api/uplink/config
  it('读取上行配置走 GET /api/uplink/config', async () => {
    const cfg = uplinkCfg()
    requestMock.mockResolvedValue(cfg)

    await expect(getUplinkConfig()).resolves.toEqual(cfg)

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/uplink/config')
  })

  // 锁定保存契约：POST 同路径（不是 PUT），请求体原样透传整个配置
  it('保存上行配置走 POST /api/uplink/config，请求体原样透传', async () => {
    const cfg = uplinkCfg({ max_bytes: 8192 })
    requestMock.mockResolvedValue(cfg)

    await saveUplinkConfig(cfg)

    const c = firstCallConfig()
    expect(c.method).toBe('POST')
    expect(c.url).toBe('/api/uplink/config')
    expect(c.data).toEqual(cfg)
  })

  // 锁定模板变量契约：GET /api/uplink/vars
  it('列出模板变量走 GET /api/uplink/vars', async () => {
    const vars = [{ name: 'value', description: '点位值', type: 'number' as const }]
    requestMock.mockResolvedValue(vars)

    await expect(listTemplateVars()).resolves.toEqual(vars)

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/uplink/vars')
  })
})

describe('uplink 上送测试契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定测试契约：POST /api/uplink/test，请求体 { template, vars, send }
  it('测试发送走 POST /api/uplink/test，请求体为 { template, vars, send }', async () => {
    requestMock.mockResolvedValue({ ok: true, rendered: '{"v":1}', bytes: 8 })

    const res = await testUplink({ template: '{"v":${value}}', vars: { value: 1 }, send: false })

    const c = firstCallConfig()
    expect(c.method).toBe('POST')
    expect(c.url).toBe('/api/uplink/test')
    expect(c.data).toEqual({ template: '{"v":${value}}', vars: { value: 1 }, send: false })
    expect(res.ok).toBe(true)
  })

  // 锁定失败分支：后端返回 ok=false + error 时原样透传，不在 api 层吞掉
  it('测试失败结果原样透传（ok=false + error）', async () => {
    const resp = { ok: false, rendered: '', bytes: 0, error: '模板编译失败: 缺少 }' }
    requestMock.mockResolvedValue(resp)

    await expect(testUplink({ template: 'bad', vars: {}, send: false })).resolves.toEqual(resp)
  })
})

describe('uplink 模板导入导出与测试历史契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定导出契约：GET /api/uplink/template/export
  it('导出模板走 GET /api/uplink/template/export', async () => {
    const exported = { template: '{}', enabled: true, encoding: 'json', max_bytes: 4096, exported_at: '2026-01-01T00:00:00Z' }
    requestMock.mockResolvedValue(exported)

    await expect(exportTemplate()).resolves.toEqual(exported)

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/uplink/template/export')
  })

  // 锁定导入契约：POST /api/uplink/template/import，请求体只含 template + enabled
  it('导入模板走 POST /api/uplink/template/import，请求体只含 template 与 enabled', async () => {
    requestMock.mockResolvedValue(uplinkCfg())

    await importTemplate({ template: '{"v":1}', enabled: false })

    const c = firstCallConfig()
    expect(c.method).toBe('POST')
    expect(c.url).toBe('/api/uplink/template/import')
    expect(c.data).toEqual({ template: '{"v":1}', enabled: false })
  })

  // 锁定历史列表契约：GET /api/uplink/test/history?limit=N（limit 进 query string，默认 50）
  it('列出测试历史走 GET /api/uplink/test/history?limit=50（默认 50 条）', async () => {
    requestMock.mockResolvedValue([])

    await listTestHistory()

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/uplink/test/history?limit=50')
  })

  it('测试历史条数可覆盖（limit 进 query string）', async () => {
    requestMock.mockResolvedValue([])

    await listTestHistory(200)

    expect(firstCallConfig().url).toBe('/api/uplink/test/history?limit=200')
  })

  // 锁定清空契约：DELETE /api/uplink/test/history（与列表同路径，靠 method 区分）
  it('清空测试历史走 DELETE /api/uplink/test/history', async () => {
    requestMock.mockResolvedValue({ cleared: true })

    const res = await clearTestHistory()

    const c = firstCallConfig()
    expect(c.method).toBe('DELETE')
    expect(c.url).toBe('/api/uplink/test/history')
    expect(res).toEqual({ cleared: true })
  })
})

describe('uplink 透传会话与北向状态契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定透传契约：GET /api/passthrough/sessions
  it('查询透传会话走 GET /api/passthrough/sessions', async () => {
    requestMock.mockResolvedValue({ enabled: true, sessions: [] })

    await getPassthroughSessions()

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/passthrough/sessions')
  })

  // 锁定北向状态契约：GET /api/north/status
  it('查询北向状态走 GET /api/north/status', async () => {
    requestMock.mockResolvedValue({ healthy: true, channels: [], aggregated: {} })

    await getNorthStatus()

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/north/status')
  })

  // 第 1 期冗余收口：`GET /api/north/servers` 已删除，
  // Server 存活列表并入 `GET /api/north/status` 的 servers 字段。
  it('北向 Server 列表随 /api/north/status 一并返回，不再请求 /api/north/servers', async () => {
    const servers = [{ name: 'opcua', running: true }]
    requestMock.mockResolvedValue({ healthy: true, channels: [], aggregated: {}, servers })

    const status = await getNorthStatus()

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/north/status')
    expect(status.servers).toEqual(servers)
    expect(requestMock).toHaveBeenCalledTimes(1)
  })
})

describe('uplink 内置模板库', () => {
  // 锁定内置模板契约：id 唯一、模板串含占位符，供下拉直接选用
  it('内置模板 id 唯一且模板含占位符', () => {
    const ids = BUILTIN_TEMPLATES.map((t) => t.id)
    expect(new Set(ids).size).toBe(ids.length)
    expect(ids).toContain('default')
    BUILTIN_TEMPLATES.forEach((t) => {
      expect(t.template).toContain('${')
      expect(t.name.length).toBeGreaterThan(0)
    })
  })
})
