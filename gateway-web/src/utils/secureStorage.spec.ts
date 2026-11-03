/**
 * secureStorage 单元测试
 *
 * 覆盖：
 * - 加密 / 解密往返一致性
 * - 老明文 token 自动迁移为密文
 * - 已经是密文时不再重复迁移
 * - 解密失败时清掉过期密文条目
 * - clearToken 同步生效
 * - 多次 setToken 后 key 复用（不应该每次都生成新 key，否则老 token 解不开）
 *
 * 测试环境：
 * - jsdom（vitest.config.ts）
 * - fake-indexeddb（vitest.setup.ts 已注册，见下方 import）
 * - Node 22+ 内置 webcrypto（setup.ts 里 polyfill 到 globalThis）
 */
import { describe, it, expect, beforeEach } from 'vitest'
import 'fake-indexeddb/auto'
import { setToken, getToken, clearToken, migrateLegacyToken } from '../utils/secureStorage'

const LEGACY_KEY = 'gateway_api_token'

beforeEach(async () => {
  // 每个测试前清掉 localStorage 和 IndexedDB，确保互不干扰
  localStorage.clear()
  // fake-indexeddb 提供 indexedDB 全局，但每个测试用的是同一个 fake DB；
  // 这里清掉我们的 store 数据
  const dbs = await indexedDB.databases?.()
  if (dbs) {
    for (const db of dbs) {
      if (db.name) indexedDB.deleteDatabase(db.name)
    }
  }
})

describe('secureStorage — AES-GCM 加密往返', () => {
  it('setToken 后 localStorage 写入密文（enc: 前缀）', async () => {
    await setToken('plain-secret-123')
    const raw = localStorage.getItem(LEGACY_KEY)
    expect(raw).not.toBeNull()
    expect(raw!.startsWith('enc:')).toBe(true)
    // 密文不应包含明文
    expect(raw!.includes('plain-secret-123')).toBe(false)
  })

  it('getToken 解密后能拿到原文', async () => {
    await setToken('plain-secret-123')
    const got = await getToken()
    expect(got).toBe('plain-secret-123')
  })

  it('多次 setToken 不同值都能正确解密（key 复用）', async () => {
    await setToken('first-token')
    expect(await getToken()).toBe('first-token')

    await setToken('second-token-different')
    expect(await getToken()).toBe('second-token-different')
  })

  it('clearToken 后 getToken 返回空字符串', async () => {
    await setToken('to-be-cleared')
    clearToken()
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull()
    expect(await getToken()).toBe('')
  })

  it('localStorage 中无值时 getToken 返回空字符串', async () => {
    expect(await getToken()).toBe('')
  })
})

describe('secureStorage — 老明文 token 迁移', () => {
  it('migrateLegacyToken 把明文加密为密文', async () => {
    localStorage.setItem(LEGACY_KEY, 'old-plaintext-token')
    const migrated = await migrateLegacyToken()
    expect(migrated).toBe(true)
    const raw = localStorage.getItem(LEGACY_KEY)
    expect(raw!.startsWith('enc:')).toBe(true)
    expect(raw!.includes('old-plaintext-token')).toBe(false)
    // 迁移后能正常解密
    expect(await getToken()).toBe('old-plaintext-token')
  })

  it('已经是 enc: 密文时 migrateLegacyToken 不重复迁移', async () => {
    await setToken('already-encrypted')
    const before = localStorage.getItem(LEGACY_KEY)
    const migrated = await migrateLegacyToken()
    expect(migrated).toBe(false)
    expect(localStorage.getItem(LEGACY_KEY)).toBe(before)
  })

  it('localStorage 空时 migrateLegacyToken 返回 false', async () => {
    const migrated = await migrateLegacyToken()
    expect(migrated).toBe(false)
  })
})

describe('secureStorage — 解密失败容错', () => {
  it('篡改密文后 getToken 返回空字符串并清掉条目', async () => {
    await setToken('legit-token')
    // 篡改 ct 字段 — 解密必然失败
    const raw = localStorage.getItem(LEGACY_KEY)!
    const payload = JSON.parse(raw.slice('enc:'.length))
    payload.ct = 'AAAA' // 故意改成无效 base64 + 不匹配内容
    localStorage.setItem(LEGACY_KEY, 'enc:' + JSON.stringify(payload))

    const got = await getToken()
    expect(got).toBe('')
    // 失败后条目被清，等同于强制登出
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull()
  })

  it('篡改 IV 后同样解密失败并清条目', async () => {
    await setToken('another-legit')
    const raw = localStorage.getItem(LEGACY_KEY)!
    const payload = JSON.parse(raw.slice('enc:'.length))
    payload.iv = 'AAAA'
    localStorage.setItem(LEGACY_KEY, 'enc:' + JSON.stringify(payload))

    expect(await getToken()).toBe('')
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull()
  })

  it('JSON 损坏的 enc: 条目 → 静默清掉', async () => {
    localStorage.setItem(LEGACY_KEY, 'enc:not-json{{{')
    expect(await getToken()).toBe('')
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull()
  })
})

describe('secureStorage — 边界场景', () => {
  it('空字符串 token 也能加解密', async () => {
    await setToken('')
    // 空串会被视为"无 token"；测试加解密本身
    const raw = localStorage.getItem(LEGACY_KEY)
    // setToken('') 仍会写入，密文存在
    expect(raw).not.toBeNull()
    expect(raw!.startsWith('enc:')).toBe(true)
    // getToken('') 返回 ''
    expect(await getToken()).toBe('')
  })

  it('长 token（>1KB）能加解密', async () => {
    const long = 'x'.repeat(2048)
    await setToken(long)
    expect(await getToken()).toBe(long)
  })

  it('包含特殊字符的 token 能加解密', async () => {
    const special = 'tok"en\\with/特殊字符\nand\tcontrol\u0000chars'
    await setToken(special)
    expect(await getToken()).toBe(special)
  })
})
