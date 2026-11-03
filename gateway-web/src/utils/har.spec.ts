import { describe, it, expect } from 'vitest'
import { buildHar, buildHarObject } from './har'
import type { RequestLogEntry } from '@/stores/requestLog'

/** 构造一条请求日志条目 */
function entry(over: Partial<RequestLogEntry> = {}): RequestLogEntry {
  return {
    id: 1,
    method: 'GET',
    url: '/api/devices',
    status: 200,
    duration: 12,
    timestamp: 1_700_000_000_000,
    startTime: 1_700_000_000_000,
    endTime: 1_700_000_000_012,
    ...over,
  }
}

/**
 * HAR 导出的回归护栏（P1-12）。
 *
 * 背景：原实现 request/response 的 headers 恒为 `[]`，导出的 HAR 拖进
 * Chrome DevTools Network 面板后没有 Content-Type / Accept 等头，
 * 回放必然失败——「导出了 HAR 却用不了」。
 */
describe('buildHar（HAR 1.2 导出）', () => {
  it('请求/响应头不再恒空：写入实际记录到的头', () => {
    const har = buildHarObject([
      entry({
        requestHeaders: [
          { name: 'Accept', value: 'application/json' },
          { name: 'Content-Type', value: 'application/json' },
        ],
        responseHeaders: [{ name: 'content-type', value: 'application/json' }],
      }),
    ])
    const e = har.log.entries[0]!
    expect(e.request.headers).toEqual([
      { name: 'Accept', value: 'application/json' },
      { name: 'Content-Type', value: 'application/json' },
    ])
    expect(e.response.headers).toEqual([{ name: 'content-type', value: 'application/json' }])
  })

  it('headersSize 不再是 -1：有头时按实际长度给值', () => {
    const har = buildHarObject([
      entry({ requestHeaders: [{ name: 'Accept', value: 'application/json' }] }),
    ])
    const e = har.log.entries[0]!
    // name.length + value.length + 4（": " 与 CRLF 粗算）
    expect(e.request.headersSize).toBe('Accept'.length + 'application/json'.length + 4)
    expect(e.request.headersSize).toBeGreaterThan(0)
  })

  it('未记录头时给空数组（HAR 规范要求该字段存在），不报错', () => {
    const har = buildHarObject([entry()])
    const e = har.log.entries[0]!
    expect(e.request.headers).toEqual([])
    expect(e.response.headers).toEqual([])
  })

  it('queryString 从 URL 解析出来（HAR 规范要求，否则 DevTools 看不到参数）', () => {
    const har = buildHarObject([entry({ url: '/api/points?device_id=d1&limit=100' })])
    const e = har.log.entries[0]!
    expect(e.request.queryString).toEqual([
      { name: 'device_id', value: 'd1' },
      { name: 'limit', value: '100' },
    ])
  })

  it('无 query 的 URL：queryString 为空数组', () => {
    const har = buildHarObject([entry({ url: '/api/devices' })])
    expect(har.log.entries[0]!.request.queryString).toEqual([])
  })

  it('WS 帧带 _ws 自定义字段，不影响 HAR 兼容性', () => {
    const har = buildHarObject([
      entry({
        kind: 'ws',
        method: 'WS',
        url: '/api/realtime',
        status: 101,
        ws: { direction: 'in', type: 'data', size: 10, data: '{}' },
      }),
    ])
    const e = har.log.entries[0] as unknown as { _ws?: unknown }
    expect(e._ws).toEqual({ direction: 'in', type: 'data', size: 10, data: '{}' })
  })

  it('buildHar 输出可解析的 JSON 文本', () => {
    const text = buildHar([entry({ requestHeaders: [{ name: 'Accept', value: '*/*' }] })])
    const parsed = JSON.parse(text)
    expect(parsed.log.version).toBe('1.2')
    expect(parsed.log.entries[0].request.headers).toHaveLength(1)
  })
})
