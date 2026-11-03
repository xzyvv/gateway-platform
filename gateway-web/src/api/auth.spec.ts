import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { login, changePassword, whoami } from './auth'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('auth（system 域登录 / whoami / 改密契约）', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定登录契约：POST /api/auth/login，字段 username/password。
  // 该端点后端不走 ApiResponse 信封（裸 { token }），前端也只透传不改形。
  it('login 走 POST /api/auth/login，请求体为 { username, password }', async () => {
    requestMock.mockResolvedValue({ token: 'tok-123' })

    await login('admin', 'secret')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/auth/login')
    expect(cfg.data).toEqual({ username: 'admin', password: 'secret' })
  })

  it('login 原样返回后端 token（不额外包一层）', async () => {
    requestMock.mockResolvedValue({ token: 'tok-123' })

    await expect(login('admin', 'secret')).resolves.toEqual({ token: 'tok-123' })
  })

  // 锁定改密契约：POST /api/auth/password，字段是 snake_case 的
  // old_password / new_password（后端 ChangePasswordRequest 未开 camelCase）。
  // 一旦写成 oldPassword，后端 serde 反序列化失败 → 改密永远 400。
  it('changePassword 走 POST /api/auth/password，请求体为 snake_case 字段', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })

    await changePassword('old-pass', 'new-pass')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/auth/password')
    expect(cfg.data).toEqual({ old_password: 'old-pass', new_password: 'new-pass' })
    expect((cfg.data as Record<string, unknown>).oldPassword).toBeUndefined()
  })

  // 锁定 whoami 契约：GET /api/auth/whoami（路由守卫 / 启动时调用）
  it('whoami 走 GET /api/auth/whoami 且不带请求体', async () => {
    requestMock.mockResolvedValue({
      username: 'admin',
      using_default_password: true,
      security_warnings: [],
    })

    await whoami()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/auth/whoami')
    expect(cfg.data).toBeUndefined()
  })

  // 锁定 whoami 返回值原样透传：using_default_password / security_warnings /
  // credential / scopes 都要能被调用方读到（前端据此判「是否仍在用默认密码」）。
  it('whoami 原样透传身份与安全告警字段', async () => {
    const resp = {
      username: 'admin',
      using_default_password: true,
      security_warnings: ['默认凭据', '开放 CORS'],
      credential: 'primary',
      scopes: ['*'],
    }
    requestMock.mockResolvedValue(resp)

    await expect(whoami()).resolves.toEqual(resp)
  })

  // 错误分支：401（token 失效）必须 reject，调用方据此清 token 跳登录页
  it('whoami 遇到 401 时原样 reject（由调用方决定清 token）', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(401): token 已失效'))

    await expect(whoami()).rejects.toThrow('token 已失效')
  })

  it('login 凭据错误时原样 reject，不在 api 层吞掉', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(401): 用户名或密码错误'))

    await expect(login('admin', 'wrong')).rejects.toThrow('用户名或密码错误')
  })
})
