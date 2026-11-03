/**
 * SSE 流式请求共享件（p0-1b 回放 / p0-2 从站扫描 共用）
 *
 * 实现要点（契约 p0-1b §6 / p0-2 §2，两端点一致）：
 * - 用 fetch + ReadableStream 解析 SSE，不引入 EventSource（它无法带 Bearer 头）；
 * - 绕过 axios 拦截器（SSE 不是 ApiResponse JSON 包装），鉴权头显式附加；
 * - 准入失败（404/400/bus_busy/bus_offline）在流建立前返回 JSON 错误体，
 *   解析其 message 抛出，与 axios 路径的现场指引一致；
 * - 用户 abort（signal.aborted）属正常收尾（契约 §4），resolve 而非 reject。
 */
import { getToken } from '../token'
import { isTauri } from '../env'

/** 流式端点事件回调：event 为 SSE `event:` 字段，data 为单行 JSON 解析结果 */
export interface SseStreamHandlers {
  onEvent: (event: string, data: Record<string, unknown>) => void
  /** 用户中断（AbortController）：后端在下一帧/下一地址前停止，本函数正常 resolve 不 reject */
  signal?: AbortSignal
}

/**
 * 当前环境是否可用 SSE 流式请求（不可用时调用方应回落阻塞模式）。
 * Tauri 桌面端相对 URL 的 fetch 不会指向网关（axios 走 tauriAdapter），按不可用处理。
 */
export function canUseSseStream(): boolean {
  return typeof fetch === 'function' && typeof ReadableStream === 'function' && !isTauri()
}

/**
 * POST + Accept: text/event-stream 发起流式请求，逐事件回调。
 * @param errPrefix 准入失败且无服务端 message 时的错误文案前缀
 */
export async function postSseStream(
  url: string,
  body: unknown,
  handlers: SseStreamHandlers,
  errPrefix: string,
): Promise<void> {
  const token = await getToken()
  try {
    const resp = await fetch(url, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Accept: 'text/event-stream',
        ...(token ? { Authorization: `Bearer ${token}` } : {}),
      },
      body: JSON.stringify(body),
      signal: handlers.signal ?? null,
    })
    const contentType = resp.headers.get('content-type') ?? ''
    if (!resp.ok || !contentType.includes('text/event-stream') || !resp.body) {
      const errBody = (await resp.json().catch(() => null)) as { message?: string } | null
      throw new Error(errBody?.message || `${errPrefix}（HTTP ${resp.status}）`)
    }

    const reader = resp.body.getReader()
    const decoder = new TextDecoder()
    let buffer = ''
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      // SSE 事件以空行分隔；逐块提取，容忍 TCP 粘包/拆包
      let sep: number
      while ((sep = buffer.indexOf('\n\n')) >= 0) {
        const raw = buffer.slice(0, sep)
        buffer = buffer.slice(sep + 2)
        dispatchSseBlock(raw, handlers.onEvent)
      }
    }
  } catch (e) {
    if (handlers.signal?.aborted) return // 用户主动停止：正常收尾，不算失败
    throw e
  }
}

/** 解析单个 SSE 事件块并回调；心跳注释行（`:...`）按契约无需处理；半截 JSON 容错跳过 */
function dispatchSseBlock(raw: string, onEvent: SseStreamHandlers['onEvent']): void {
  let event = ''
  const dataLines: string[] = []
  for (const line of raw.split('\n')) {
    if (line.startsWith(':')) continue
    if (line.startsWith('event:')) event = line.slice(6).trim()
    else if (line.startsWith('data:')) dataLines.push(line.slice(5).replace(/^ /, ''))
  }
  if (!event || dataLines.length === 0) return
  try {
    onEvent(event, JSON.parse(dataLines.join('\n')) as Record<string, unknown>)
  } catch {
    // 半截 JSON 容错：跳过该事件，不中断流
  }
}
