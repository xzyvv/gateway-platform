/**
 * 响应元信息（trace ID / 离线陈旧标记）测试。
 *
 * - readTraceId：后端对每个响应写入 `x-request-id`，任何失败分支都要能拿到。
 * - readResponseMeta：Service Worker 离线回退时注入 `X-Data-Stale` / `X-Data-Age`，
 *   页面据此区分「真实新数据」与「离线旧数据」。
 */
import { describe, it, expect } from 'vitest'
import { readTraceId, readResponseMeta } from './trace'

describe('readTraceId（x-request-id）', () => {
  it('AxiosHeaders（get 方法）与普通对象都能读取，空白被裁掉', () => {
    expect(readTraceId({ get: (n: string) => (n === 'x-request-id' ? 'abc123' : null) })).toBe('abc123')
    expect(readTraceId({ 'x-request-id': ' web-1 ' })).toBe('web-1')
  })

  it('数组形式取第一个值', () => {
    expect(readTraceId({ 'x-request-id': ['id-1', 'id-2'] })).toBe('id-1')
  })

  it('缺失 / 空串 / 非字符串返回 undefined', () => {
    expect(readTraceId(undefined)).toBeUndefined()
    expect(readTraceId(null)).toBeUndefined()
    expect(readTraceId({})).toBeUndefined()
    expect(readTraceId({ 'x-request-id': '   ' })).toBeUndefined()
    expect(readTraceId({ 'x-request-id': 123 })).toBeUndefined()
  })
})

describe('readResponseMeta（离线陈旧标记）', () => {
  it('注入 stale=1 与 age 时如实读出', () => {
    expect(readResponseMeta({ 'x-data-stale': '1', 'x-data-age': '120' })).toEqual({
      stale: true,
      staleAgeSeconds: 120,
    })
  })

  it('无任何标记时既不陈旧也无年龄', () => {
    expect(readResponseMeta({})).toEqual({ stale: false, staleAgeSeconds: null })
    expect(readResponseMeta(undefined)).toEqual({ stale: false, staleAgeSeconds: null })
  })

  it('stale 只有恰好为 "1" 才算陈旧', () => {
    expect(readResponseMeta({ 'x-data-stale': '0' }).stale).toBe(false)
    expect(readResponseMeta({ 'x-data-stale': 'true' }).stale).toBe(false)
    expect(readResponseMeta({ 'x-data-stale': '1' }).stale).toBe(true)
  })

  it('年龄取整；负数归零；非法值返回 null', () => {
    expect(readResponseMeta({ 'x-data-age': '12.6' }).staleAgeSeconds).toBe(13)
    expect(readResponseMeta({ 'x-data-age': '-5' }).staleAgeSeconds).toBe(0)
    expect(readResponseMeta({ 'x-data-age': 'abc' }).staleAgeSeconds).toBe(null)
    expect(readResponseMeta({ 'x-data-age': '' }).staleAgeSeconds).toBe(null)
  })
})
