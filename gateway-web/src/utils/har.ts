/**
 * HAR 1.2（HTTP Archive）构造
 *
 * 从请求监视器里抽出来共用：单页「导出 HAR」与「一键诊断包」里的 requests.har
 * 必须是同一份格式，否则同一批请求在两处导出结果不一致，研发对日志时容易误判。
 *
 * WS 帧没有标准 HAR 表示，作为自定义 `_ws` 字段附加，不影响 HAR 兼容性
 * （DevTools 会忽略未知字段，仍可正常导入回放 HTTP 部分）。
 *
 * 修复（P1-12）：原先 request/response 的 headers 恒为 `[]`——拖进 Chrome DevTools
 * Network 面板后没有 Content-Type / Accept 等头，回放直接失败。现在从请求日志条目
 * 里取实际记录到的头（见 stores/requestLog.ts 的 requestHeaders/responseHeaders）。
 */
import type { RequestLogEntry } from '@/stores/requestLog'

function bodyText(v: unknown): string | undefined {
  if (v === undefined) return undefined
  return typeof v === 'string' ? v : JSON.stringify(v)
}

/** HAR 头数组：[{ name, value }]，缺省给空数组（HAR 规范要求该字段存在） */
function harHeaders(
  headers: Array<{ name: string; value: string }> | undefined,
): Array<{ name: string; value: string }> {
  return headers ?? []
}

/** URL 的 query 拆成 HAR queryString 数组 */
function harQueryString(url: string): Array<{ name: string; value: string }> {
  const q = url.indexOf('?')
  if (q < 0) return []
  const out: Array<{ name: string; value: string }> = []
  for (const pair of url.slice(q + 1).split('&')) {
    if (!pair) continue
    const eq = pair.indexOf('=')
    if (eq < 0) {
      out.push({ name: pair, value: '' })
    } else {
      out.push({ name: pair.slice(0, eq), value: pair.slice(eq + 1) })
    }
  }
  return out
}

/** 构造 HAR 对象（尚未序列化，便于调用方嵌入其它结构） */
export function buildHarObject(entries: RequestLogEntry[]) {
  const harEntries = entries.map((r) => {
    const responseText = bodyText(r.responseData) ?? ''
    const requestText = bodyText(r.requestData)
    const reqHeaders = harHeaders(r.requestHeaders)
    const resHeaders = harHeaders(r.responseHeaders)
    // headerSize：HAR 规范为「头字节数，未知时 -1」。有记录头时按 name+": "+value 粗算，
    // 给 0 会让部分解析器认为没有头。
    const headerSize = (hs: Array<{ name: string; value: string }>) =>
      hs.reduce((sum, h) => sum + h.name.length + h.value.length + 4, 0)
    const base = {
      startedDateTime: new Date(r.startTime).toISOString(),
      time: r.duration,
      request: {
        method: r.method,
        url: r.url,
        httpVersion: 'HTTP/1.1',
        headers: reqHeaders,
        queryString: harQueryString(r.url),
        cookies: [] as unknown[],
        headersSize: headerSize(reqHeaders),
        bodySize: requestText ? requestText.length : 0,
        postData: requestText !== undefined
          ? { mimeType: 'application/json', text: requestText }
          : undefined,
      },
      response: {
        status: typeof r.status === 'number' ? r.status : 0,
        statusText: String(r.status),
        httpVersion: 'HTTP/1.1',
        headers: resHeaders,
        cookies: [] as unknown[],
        content: {
          size: responseText.length,
          mimeType: 'application/json',
          text: responseText,
        },
        redirectURL: '',
        headersSize: headerSize(resHeaders),
        bodySize: responseText.length,
      },
      cache: {},
      timings: { send: 0, wait: r.duration, receive: 0 },
    }
    if (r.kind === 'ws' && r.ws) {
      ;(base as Record<string, unknown>)._ws = r.ws
    }
    return base
  })

  return {
    log: {
      version: '1.2',
      creator: { name: 'gateway-web RequestMonitor', version: '1.0' },
      entries: harEntries,
    },
  }
}

/** 构造 HAR 文本（带两空格缩进，便于直接肉眼查） */
export function buildHar(entries: RequestLogEntry[]): string {
  return JSON.stringify(buildHarObject(entries), null, 2)
}
