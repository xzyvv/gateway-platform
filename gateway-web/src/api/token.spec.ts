import { describe, it, expect, vi, beforeEach } from 'vitest'

// token.ts 只是 secureStorage 的薄封装：真正的加密逻辑在 ../utils/secureStorage，
// 这里 mock 掉存储层，锁「api 层对 secureStorage 的调用契约 + 返回值透传」。
const { secureMock } = vi.hoisted(() => ({
  secureMock: {
    getToken: vi.fn(),
    setToken: vi.fn(),
    clearToken: vi.fn(),
    migrateLegacyToken: vi.fn(),
  },
}))
vi.mock('../utils/secureStorage', () => secureMock)

import { getToken, setToken, clearToken, migrateLegacyToken } from './token'

describe('token（system 域 token 本地存储契约）', () => {
  beforeEach(() => {
    secureMock.getToken.mockReset()
    secureMock.setToken.mockReset()
    secureMock.clearToken.mockReset()
    secureMock.migrateLegacyToken.mockReset()
  })

  // 锁定读契约：getToken 必须走 secureStorage.getToken 并原样返回其结果
  it('getToken 走 secureStorage.getToken 并原样返回', async () => {
    secureMock.getToken.mockResolvedValue('tok-abc')

    await expect(getToken()).resolves.toBe('tok-abc')
    expect(secureMock.getToken).toHaveBeenCalledTimes(1)
  })

  // 边界：无 token 时 secureStorage 返回空串，api 层不得转成 null/undefined
  it('getToken 在无 token 时透传空字符串', async () => {
    secureMock.getToken.mockResolvedValue('')

    const v = await getToken()
    expect(v).toBe('')
    expect(v).not.toBeNull()
  })

  // 锁定写契约：setToken 把 token 原样交给 secureStorage.setToken 并等待其完成
  it('setToken 调用 secureStorage.setToken 并透传入参', async () => {
    secureMock.setToken.mockResolvedValue(undefined)

    await setToken('new-token')

    expect(secureMock.setToken).toHaveBeenCalledTimes(1)
    expect(secureMock.setToken).toHaveBeenCalledWith('new-token')
  })

  // 边界：setToken 失败（加密异常）必须 reject，调用方据此不误报「已登录」
  it('setToken 失败时原样 reject', async () => {
    secureMock.setToken.mockRejectedValue(new Error('encrypt failed'))

    await expect(setToken('bad')).rejects.toThrow('encrypt failed')
  })

  // 锁定清除契约：clearToken 是同步 API（返回 void），只调 secureStorage.clearToken
  it('clearToken 是同步调用并透传给 secureStorage.clearToken', () => {
    secureMock.clearToken.mockReturnValue(undefined)

    const r = clearToken()

    expect(r).toBeUndefined()
    expect(secureMock.clearToken).toHaveBeenCalledTimes(1)
  })

  // 锁定迁移契约：migrateLegacyToken 走动态 import 调用 secureStorage 的同名函数，
  // 并把 boolean 结果透传给调用方（true=确有老明文需要迁移）。
  it('migrateLegacyToken 调用 secureStorage.migrateLegacyToken 并透传布尔结果', async () => {
    secureMock.migrateLegacyToken.mockResolvedValue(true)

    await expect(migrateLegacyToken()).resolves.toBe(true)
    expect(secureMock.migrateLegacyToken).toHaveBeenCalledTimes(1)
  })

  it('migrateLegacyToken 在无需迁移时返回 false', async () => {
    secureMock.migrateLegacyToken.mockResolvedValue(false)

    await expect(migrateLegacyToken()).resolves.toBe(false)
  })

  // 错误分支：迁移异常原样 reject（不吞掉，启动期据此提示降级）
  it('migrateLegacyToken 失败时原样 reject', async () => {
    secureMock.migrateLegacyToken.mockRejectedValue(new Error('idb read failed'))

    await expect(migrateLegacyToken()).rejects.toThrow('idb read failed')
  })
})
