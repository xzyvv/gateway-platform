/**
 * 日志脱敏模块（sanitize）内部辅助函数测试。
 *
 * 覆盖点：
 * - isPlainObject：只认「纯对象」，Date / Map / 类实例 / 数组都不算。
 * - loggableBody：二进制体（FormData / Blob / ArrayBuffer）直接省略，
 *   避免私钥 / 证书内容落进 requestLog。
 *
 * 说明：sanitizeForLog / sanitizeUrl / loggableHeaders 的行为由
 * `src/api/http.spec.ts` 覆盖（走 barrel 导入），此处只补模块内部辅助函数。
 */
import { describe, it, expect } from 'vitest'
import { isPlainObject, loggableBody } from './sanitize'

describe('isPlainObject（纯对象判定）', () => {
  it('对象字面量与 null 原型对象算纯对象', () => {
    expect(isPlainObject({})).toBe(true)
    expect(isPlainObject({ a: 1 })).toBe(true)
    expect(isPlainObject(Object.create(null))).toBe(true)
  })

  it('null / 非对象 / 数组 / Date / 类实例都不算纯对象', () => {
    expect(isPlainObject(null)).toBe(false)
    expect(isPlainObject(undefined)).toBe(false)
    expect(isPlainObject(1)).toBe(false)
    expect(isPlainObject('s')).toBe(false)
    expect(isPlainObject([])).toBe(false)
    expect(isPlainObject(new Date())).toBe(false)
    expect(isPlainObject(new Map())).toBe(false)
    expect(isPlainObject(new (class Foo {})())).toBe(false)
  })
})

describe('loggableBody（日志可用的请求体）', () => {
  it('undefined / null 原样返回', () => {
    expect(loggableBody('GET', '/api/x', undefined)).toBeUndefined()
    expect(loggableBody('GET', '/api/x', null)).toBeNull()
  })

  it('普通对象按敏感字段名脱敏', () => {
    expect(loggableBody('POST', '/api/config', { username: 'admin', password: 'p' })).toEqual({
      username: 'admin',
      password: '***',
    })
  })

  it('FormData / Blob / ArrayBuffer 直接省略，不落明文内容', () => {
    expect(loggableBody('POST', '/api/cert', new FormData())).toBe('[form-data omitted]')
    expect(loggableBody('POST', '/api/cert', new Blob(['secret']))).toBe('[blob omitted]')
    expect(loggableBody('POST', '/api/cert', new ArrayBuffer(8))).toBe('[binary omitted]')
  })

  it('数组按元素逐个脱敏', () => {
    expect(loggableBody('POST', '/api/x', [{ token: 't' }])).toEqual([{ token: '***' }])
  })
})
