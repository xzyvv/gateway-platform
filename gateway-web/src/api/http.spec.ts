/**
 * W-4 修复配套测试：sanitizeForLog 把含 password / token / secret 等字段的请求体
 * 递归替换为 '***'，保证 requestLog store 不会持久化明文凭据。
 */
import { describe, it, expect, afterEach } from 'vitest'
import axios from 'axios'
import { sanitizeForLog, sanitizeUrl, readTraceId, instance, loggableHeaders } from './http'
import { abortAllInflight, getInflightCount } from './inflight'

describe('sanitizeForLog（W-4 脱敏）', () => {
  it('顶层 password / token / secret 字段被替换', () => {
    const out = sanitizeForLog({
      username: 'admin',
      password: 'P@ssw0rd',
      api_token: 'eyJhbGciOi...',
      secret: 'shhh',
    })
    expect(out).toEqual({
      username: 'admin',
      password: '***',
      api_token: '***',
      secret: '***',
    })
  })

  it('嵌套对象内敏感字段也命中', () => {
    // 注意：容器字段名不能再叫 tokens——W-5 子串匹配后 "tokens" 自身就命中
    // "token"，整个容器会被替换为 '***'。这里换个中性名以覆盖"递归进容器"的行为。
    const out = sanitizeForLog({
      user: { name: 'a', password: '123', nested: { refresh_token: 'xyz' } },
      meta: { public: 'ok' },
    })
    expect(out).toEqual({
      user: { name: 'a', password: '***', nested: { refresh_token: '***' } },
      meta: { public: 'ok' },
    })
  })

  it('数组里每个对象也命中', () => {
    const out = sanitizeForLog({
      users: [
        { id: 1, password: 'a' },
        { id: 2, password_hash: 'b' },
      ],
    })
    expect(out).toEqual({
      users: [
        { id: 1, password: '***' },
        { id: 2, password_hash: '***' },
      ],
    })
  })

  it('非敏感字段保持原值（含数字、布尔、null）', () => {
    const out = sanitizeForLog({ n: 1, b: true, z: null, s: 'plain' })
    expect(out).toEqual({ n: 1, b: true, z: null, s: 'plain' })
  })

  it('非对象值原样返回（字符串 / undefined / 数字）', () => {
    expect(sanitizeForLog('hello')).toBe('hello')
    expect(sanitizeForLog(undefined)).toBeUndefined()
    expect(sanitizeForLog(42)).toBe(42)
  })

  it('不修改入参（深拷贝而非原地替换）', () => {
    const input = { password: 'secret', x: 1 }
    const out = sanitizeForLog(input)
    expect(input.password).toBe('secret') // 原对象未变
    expect(out).not.toBe(input)            // 返回新对象
  })

  it('循环引用安全（depth 截断，原值返回）', () => {
    const a: Record<string, unknown> = { x: 1 }
    a['self'] = a
    // 不抛异常即可
    const out = sanitizeForLog(a)
    expect(out).toBeDefined()
  })

  it('类实例 / Date 等非 plain object 字段保持引用', () => {
    const d = new Date('2026-01-01T00:00:00Z')
    const out = sanitizeForLog({ when: d, password: 'p' })
    expect(out.when).toBe(d)
    expect(out.password).toBe('***')
  })
})

describe('W-5 命名变体脱敏（子串匹配）', () => {
  it('常见命名变体全部命中（旧实现精确匹配会全部漏网）', () => {
    const input = {
      pwd: '1',
      passwd: '2',
      passphrase: '3',
      apiKey: '4',
      client_secret: '5',
      refreshToken: '6',
      Authorization: '7',
      credential: '8',
      private_key: '9',
    }
    const out = sanitizeForLog(input) as Record<string, unknown>
    for (const k of Object.keys(input)) {
      expect(out[k], `${k} 应被脱敏`).toBe('***')
    }
  })

  it('非凭据字段不受影响', () => {
    const out = sanitizeForLog({
      username: 'admin',
      deviceId: 'd1',
      meta: { public: 'ok' },
    })
    expect(out).toEqual({
      username: 'admin',
      deviceId: 'd1',
      meta: { public: 'ok' },
    })
  })
})

describe('W-4 URL 脱敏（query 中的凭据）', () => {
  it('query 中的敏感参数被替换，路径与普通参数保留', () => {
    expect(sanitizeUrl('/api/device/list?page=1&size=20')).toBe(
      '/api/device/list?page=1&size=20'
    )
    expect(sanitizeUrl('/api/auth/login?token=eyJhbGci&user=admin')).toBe(
      '/api/auth/login?token=***&user=admin'
    )
    expect(sanitizeUrl('/api/x?api_key=abc&other=1')).toBe('/api/x?api_key=***&other=1')
  })

  it('无 query / 空值安全', () => {
    expect(sanitizeUrl('/api/status')).toBe('/api/status')
    expect(sanitizeUrl(undefined)).toBe('')
    expect(sanitizeUrl('')).toBe('')
  })

  it('无值参数与畸形 query 不抛异常', () => {
    expect(sanitizeUrl('/api/x?flag&token=abc')).toBe('/api/x?flag&token=***')
    expect(sanitizeUrl('/api/x?a=1&&b=2')).toBe('/api/x?a=1&&b=2')
  })
})

describe('W-1 切网关取消「自带 signal」的在飞请求', () => {
  const originalAdapter = instance.defaults.adapter

  afterEach(() => {
    // 清理可能残留的在飞注册，避免跨用例串扰
    abortAllInflight('test cleanup')
    instance.defaults.adapter = originalAdapter
  })

  it('调用方自带 signal 的请求也会登记，并能被 abortAllInflight 取消', async () => {
    const caller = new AbortController()

    // 桩 adapter：挂起直到 signal 被 abort，模拟"请求已发出、响应未回"
    instance.defaults.adapter = (config) =>
      new Promise((_resolve, reject) => {
        const signal = config.signal as AbortSignal | undefined
        if (!signal) {
          reject(new Error('响应拦截器未给请求注入 signal'))
          return
        }
        if (signal.aborted) {
          reject(new axios.CanceledError('aborted'))
          return
        }
        signal.addEventListener('abort', () => reject(new axios.CanceledError('aborted')), {
          once: true,
        })
      })

    // 关键：调用方**自带** signal（原实现会因此跳过 addInflight）
    const pending = instance.get('/api/device/write', { signal: caller.signal })
    // 等请求拦截器里的 await getToken() 完成
    await new Promise((resolve) => setTimeout(resolve, 0))

    expect(getInflightCount()).toBe(1)

    // 切网关：必须能取消这类请求
    abortAllInflight('gateway switch')
    await expect(pending).rejects.toBeTruthy()
    expect(getInflightCount()).toBe(0)

    // 只取消内部桥接 controller，不污染调用方自己的 signal
    expect(caller.signal.aborted).toBe(false)
  })
})

describe('readTraceId（后端 x-request-id 透出）', () => {
  it('从 AxiosHeaders（有 get 方法）读取', () => {
    const headers = { get: (name: string) => (name === 'x-request-id' ? '18f2a1b3c4d5000001' : null) }
    expect(readTraceId(headers)).toBe('18f2a1b3c4d5000001')
  })

  it('从普通对象读取，并去掉首尾空白', () => {
    expect(readTraceId({ 'x-request-id': ' web-abc12345 ' })).toBe('web-abc12345')
  })

  it('数组形式取第一个值', () => {
    expect(readTraceId({ 'x-request-id': ['id-1', 'id-2'] })).toBe('id-1')
  })

  it('缺失 / 空串 / 非字符串返回 undefined（不把 undefined 写进日志）', () => {
    expect(readTraceId(undefined)).toBeUndefined()
    expect(readTraceId(null)).toBeUndefined()
    expect(readTraceId({})).toBeUndefined()
    expect(readTraceId({ 'x-request-id': '   ' })).toBeUndefined()
    expect(readTraceId({ 'x-request-id': 123 })).toBeUndefined()
  })
})

/**
 * 请求头记录与脱敏（P1-12）。
 *
 * 背景：原先完全不记录请求头，导致 HAR 的 `headers: []` 恒空、
 * 「复制为 cURL」带鉴权接口无法在终端重放。记录头之后必须同时保证：
 *   - 普通头（Content-Type 等）原样保留——它们是重放请求的必要条件；
 *   - 凭据类头（Authorization / Cookie 等）不落明文——诊断包会被邮件转发。
 */
describe('loggableHeaders（请求头记录 + 脱敏）', () => {
  it('普通头原样保留', () => {
    const out = loggableHeaders({
      Accept: 'application/json',
      'Content-Type': 'application/json',
      'x-request-id': 'abc123',
    })
    expect(out).toContainEqual({ name: 'Accept', value: 'application/json' })
    expect(out).toContainEqual({ name: 'Content-Type', value: 'application/json' })
    expect(out).toContainEqual({ name: 'x-request-id', value: 'abc123' })
  })

  it('凭据类头的值被替换为 ***（不落明文）', () => {
    const out = loggableHeaders({
      Authorization: 'Bearer eyJhbGciOiJIUzI1NiJ9.secret',
      Cookie: 'session=abc',
      'X-Api-Key': 'k-123',
    })
    for (const h of out) {
      expect(h.value, `${h.name} 不应落明文`).toBe('***')
    }
  })

  it('AxiosHeaders 形态（有 entries 方法）也能解析', () => {
    const out = loggableHeaders({
      entries: () => [['Accept', 'application/json'], ['Authorization', 'Bearer x']],
    })
    expect(out).toContainEqual({ name: 'Accept', value: 'application/json' })
    expect(out).toContainEqual({ name: 'Authorization', value: '***' })
  })

  it('AxiosHeaders 形态（有 toJSON 方法）也能解析', () => {
    const out = loggableHeaders({
      toJSON: () => ({ 'Content-Type': 'application/json' }),
    })
    expect(out).toContainEqual({ name: 'Content-Type', value: 'application/json' })
  })

  it('超长头值被截断，避免异常头撑爆日志', () => {
    const out = loggableHeaders({ 'X-Long': 'a'.repeat(1000) })
    expect(out[0]!.value.length).toBeLessThan(600)
    expect(out[0]!.value).toContain('...(truncated)')
  })

  it('空 / 非对象输入返回空数组，不抛异常', () => {
    expect(loggableHeaders(undefined)).toEqual([])
    expect(loggableHeaders(null)).toEqual([])
    expect(loggableHeaders({})).toEqual([])
    expect(loggableHeaders('nope')).toEqual([])
  })
})
