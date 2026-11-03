import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { getAllScopes, listApiKeys, createApiKey, setApiKeyEnabled, deleteApiKey } from './apiKeys'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('apiKeys（access 域 API 密钥契约）', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定 scope 词表：必须与后端 access/handlers.rs::ALL_SCOPES 完全一致，
  // 且刻意不含 system:write —— 否则前端可自建高权密钥（提权）。
  it('getAllScopes 返回固定 7 个 scope，且不含 system:write', () => {
    expect(getAllScopes()).toEqual([
      'devices:read',
      'devices:write',
      'points:read',
      'points:write',
      'config:read',
      'config:write',
      'history:read',
    ])
    expect(getAllScopes()).not.toContain('system:write')
  })

  // 锁定列表路径：真实后端是 /api/access/api-keys（access 域前缀，不可回退成 /api/api-keys）
  it('listApiKeys 走 GET /api/access/api-keys 并原样返回列表', async () => {
    const rows = [
      {
        id: 'k1',
        name: '采集器',
        keyPreview: 'abcd1234...wxyz',
        scopes: ['devices:read'],
        enabled: true,
        createdAt: 1700000000,
        callCount: 3,
      },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listApiKeys()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/access/api-keys')
  })

  // 锁定创建请求体：字段名 expiresDays 为 camelCase（后端 serde rename_all=camelCase），
  // 一旦误写成 expires_days，后端 serde 解析不到 → 有效期静默失效（永不过期）。
  it('createApiKey 走 POST，请求体为 { name, scopes, expiresDays }', async () => {
    requestMock.mockResolvedValue({ id: 'k1', key: 'full-key-once' })

    await createApiKey('采集器', ['devices:read', 'points:read'], 30)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/access/api-keys')
    expect(cfg.data).toEqual({ name: '采集器', scopes: ['devices:read', 'points:read'], expiresDays: 30 })
    expect((cfg.data as Record<string, unknown>).expires_days).toBeUndefined()
  })

  // 边界：不传有效期时，线上报文里不能出现 expiresDays（undefined 被 JSON 丢弃），
  // 也不能退化成 0/null 之类会被后端当成「显式值」的字段。
  it('createApiKey 省略有效期时请求体不含任何有效期字段', async () => {
    requestMock.mockResolvedValue({ id: 'k1', key: 'full-key-once' })

    await createApiKey('临时', ['config:read'])

    const cfg = firstCallConfig()
    expect(cfg.data).toEqual({ name: '临时', scopes: ['config:read'] })
    expect(JSON.stringify(cfg.data)).toBe('{"name":"临时","scopes":["config:read"]}')
  })

  // 锁定启停语义：PUT + 只带 { enabled }（后端 UpdateApiKeyReq 只有 enabled 一个字段），
  // 不可改成 PATCH，也不可把整个密钥对象回传。
  it('setApiKeyEnabled 走 PUT /:id，请求体只有 { enabled }', async () => {
    requestMock.mockResolvedValue(undefined)

    await setApiKeyEnabled('k1', false)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/access/api-keys/k1')
    expect(cfg.data).toEqual({ enabled: false })
  })

  // 回归（W3a-2）：返回的是副本，调用方 push/splice 不得污染模块级常量
  it('getAllScopes 每次返回新副本，调用方改动不影响后续调用', () => {
    const first = getAllScopes()
    first.push('system:write')
    first.splice(0, 1)

    expect(getAllScopes()).toEqual([
      'devices:read',
      'devices:write',
      'points:read',
      'points:write',
      'config:read',
      'config:write',
      'history:read',
    ])
  })

  // 回归（W3a-2）：id 必须 URL 转义，否则含 `/` `?` `#` 的 id 会打错路由
  it('setApiKeyEnabled / deleteApiKey 对 id 做 URL 编码', async () => {
    requestMock.mockResolvedValue(undefined)

    await setApiKeyEnabled('a/b?c#d', true)
    expect(firstCallConfig().url).toBe('/api/access/api-keys/a%2Fb%3Fc%23d')

    requestMock.mockReset()
    requestMock.mockResolvedValue(undefined)
    await deleteApiKey('a/b?c#d')
    expect(firstCallConfig().url).toBe('/api/access/api-keys/a%2Fb%3Fc%23d')
  })

  it('deleteApiKey 走 DELETE /:id 且不带请求体', async () => {
    requestMock.mockResolvedValue(undefined)

    await deleteApiKey('k1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/access/api-keys/k1')
    expect(cfg.data).toBeUndefined()
  })

  // 错误分支：后端 400/403（名称非法、scope 未知、缺 write 权限）必须原样 reject，
  // 由页面提示「未知权限范围」等具体文案，不在 api 层吞掉。
  it('后端报错时 createApiKey 原样 reject，不做兜底', async () => {
    const err = Object.assign(new Error('请求失败(400): 未知权限范围: bad:scope'), { status: 400 })
    requestMock.mockRejectedValue(err)

    await expect(createApiKey('x', ['bad:scope'])).rejects.toThrow('未知权限范围')
    expect(requestMock).toHaveBeenCalledTimes(1)
  })
})
