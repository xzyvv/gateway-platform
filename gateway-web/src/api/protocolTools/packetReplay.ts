/**
 * 报文回放 API
 * GET /api/protocol-tools/packet-replay/sessions
 * GET /api/protocol-tools/packet-replay/sessions/:id
 * POST /api/protocol-tools/packet-replay/sessions/:id/execute
 *   - P0-1 阻塞模式（v1 §4）：缺省，一次性返回汇总
 *   - P0-1b 流式模式（Accept: text/event-stream）：SSE 逐帧推送（契约 p0-1b §2/§3）
 */
import { request } from '../index'
import { canUseSseStream, postSseStream } from './sse'
import type { ReplaySession, ReplayExecuteResult, ReplayFrameResult } from './types'
import type {
  ReplaySession as ReplaySessionRaw,
  ReplayExecuteResult as ReplayExecuteResultRaw,
  ReplayFrameResult as ReplayFrameResultRaw,
} from '../generated'
import { toReplaySession, fromReplaySession, toReplayExecuteResult, toReplayFrameResult } from './types'

export function listSessions(): Promise<ReplaySession[]> {
  return request<ReplaySessionRaw[]>({ url: '/api/protocol-tools/packet-replay/sessions', method: 'GET' }).then((arr) => arr.map(toReplaySession))
}
export function getSession(id: string): Promise<ReplaySession | undefined> {
  return request<ReplaySessionRaw | null>({ url: '/api/protocol-tools/packet-replay/sessions/' + encodeURIComponent(id), method: 'GET' }).then((r) => r ? toReplaySession(r) : undefined)
}

/** 保存或更新回放会话（预置会话不可覆盖，后端返回 400 提示）；body 需转回 snake_case */
export function saveSession(session: ReplaySession): Promise<ReplaySession> {
  return request<ReplaySessionRaw>({
    url: '/api/protocol-tools/packet-replay/sessions',
    method: 'POST',
    data: fromReplaySession(session),
  }).then(toReplaySession)
}

/** 删除持久化回放会话（预置不可删，后端返回 400） */
export function deleteSession(id: string): Promise<void> {
  return request<void>({
    url: '/api/protocol-tools/packet-replay/sessions/' + encodeURIComponent(id),
    method: 'DELETE',
  })
}

export interface ReplayExecuteOptions {
  /** 单帧超时 ms，默认后端 1500，上限 10000 */
  timeoutMs?: number
  /** 相邻帧下发间隔 ms，默认后端 200，下限 20 */
  intervalMs?: number
  /** 首个 mismatch/error 后停止，默认 false */
  stopOnMismatch?: boolean
}

function executeBody(bus: string, opts: ReplayExecuteOptions): Record<string, unknown> {
  return {
    bus,
    ...(opts.timeoutMs != null ? { timeout_ms: opts.timeoutMs } : {}),
    ...(opts.intervalMs != null ? { interval_ms: opts.intervalMs } : {}),
    ...(opts.stopOnMismatch != null ? { stop_on_mismatch: opts.stopOnMismatch } : {}),
  }
}

/**
 * 真实回放·阻塞模式（v1 契约 §4，保留）：逐条把会话中的 request 帧下发到指定总线，
 * 与录制响应对比并一次性返回逐帧结果。后端在采集占用该总线时返回 200 + error="bus_busy"，
 * 由响应拦截器透出中文 message 并 reject（不静默吞掉）。
 */
export function executeReplayBlocking(id: string, bus: string, opts: ReplayExecuteOptions = {}): Promise<ReplayExecuteResult> {
  return request<ReplayExecuteResultRaw>({
    url: '/api/protocol-tools/packet-replay/sessions/' + encodeURIComponent(id) + '/execute',
    method: 'POST',
    data: executeBody(bus, opts),
  }).then(toReplayExecuteResult)
}

/** 流式回放事件回调（P0-1b 契约 §3 事件序列：started → frame×N → summary/aborted/failed） */
export interface ReplayStreamHandlers {
  /** 租约建立后首发：携带计划总帧数，用于初始化进度条 */
  onStarted?: (info: { sessionId: string; bus: string; total: number; startedAtMs: number }) => void
  /** 单帧完成：data 即 v1 §4 ReplayFrameResult，可直接复用现有渲染逻辑 */
  onFrame?: (frame: ReplayFrameResult) => void
  /** 正常结束：全量汇总（与阻塞模式同一结构） */
  onSummary?: (result: ReplayExecuteResult) => void
  /** 服务端主动停止（如 5 分钟硬上限 reason=duration_cap），流随后关闭 */
  onAborted?: (info: { reason: string; completed: number; total: number }) => void
  /** 总线中途掉线等致命错误，流随后关闭 */
  onFailed?: (info: { error: string; message: string; completed: number; total: number }) => void
  /** 用户中断（AbortController）：后端在下一帧开始前停止，本函数正常 resolve 不 reject */
  signal?: AbortSignal
}

/**
 * 当前环境是否可用 SSE 流式回放（不可用时调用方应回落阻塞模式，契约 §6）。
 * 探测逻辑收敛到 sse.ts（p0-2 起与从站扫描共用），此处仅保留既有导出形状。
 */
export function canStreamReplay(): boolean {
  return canUseSseStream()
}

/**
 * 真实回放·流式模式（P0-1b）：Accept: text/event-stream 触发 SSE，逐帧推送。
 * 传输层（鉴权头/解析/中断语义）见 sse.ts `postSseStream`。
 */
export async function executeReplayStream(
  id: string,
  bus: string,
  opts: ReplayExecuteOptions,
  handlers: ReplayStreamHandlers,
): Promise<void> {
  await postSseStream(
    '/api/protocol-tools/packet-replay/sessions/' + encodeURIComponent(id) + '/execute',
    executeBody(bus, opts),
    {
      signal: handlers.signal,
      onEvent: (event, json) => {
        // 各事件 data 结构见契约 §3：此处在各分支内断言为具体类型
        switch (event) {
          case 'started': {
            const j = json as unknown as { session_id: string; bus: string; total: number; started_at_ms: number }
            handlers.onStarted?.({ sessionId: j.session_id, bus: j.bus, total: j.total, startedAtMs: Number(j.started_at_ms) })
            break
          }
          case 'frame':
            handlers.onFrame?.(toReplayFrameResult(json as unknown as ReplayFrameResultRaw))
            break
          case 'summary':
            handlers.onSummary?.(toReplayExecuteResult(json as unknown as ReplayExecuteResultRaw))
            break
          case 'aborted': {
            const j = json as unknown as { reason: string; completed: number; total: number }
            handlers.onAborted?.({ reason: j.reason, completed: j.completed, total: j.total })
            break
          }
          case 'failed': {
            const j = json as unknown as { error: string; message: string; completed: number; total: number }
            handlers.onFailed?.({ error: j.error, message: j.message, completed: j.completed, total: j.total })
            break
          }
        }
      },
    },
    '回放请求失败',
  )
}
