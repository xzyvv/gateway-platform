/**
 * 从站扫描 API（P0-2：真实总线扫描）
 * POST /api/protocol-tools/slave-scan
 *   - 阻塞模式（缺省）：一次性返回全量结果，响应结构与 V1 mock 同形
 *   - 流式模式（Accept: text/event-stream）：SSE 逐地址推送（契约 p0-2 §2/§4）
 */
import { request } from '../index'
import { canUseSseStream, postSseStream } from './sse'
import type { SlaveScanResult, SlaveScanStarted, SlaveScanSummary } from './types'
import type { SlaveScanResult as SlaveScanResultRaw } from '../generated'
import { toSlaveScanResult } from './types'

export function scanBus(bus: string, rangeStart: number, rangeEnd: number): Promise<SlaveScanResult[]> {
  return request<SlaveScanResultRaw[]>({ url: '/api/protocol-tools/slave-scan', method: 'POST', data: { bus, range_start: rangeStart, range_end: rangeEnd } }).then((arr) => arr.map(toSlaveScanResult))
}

/**
 * 扫描结果中「有响应」的从站。
 *
 * P0-2：`POST /api/protocol-tools/slave-scan` 是全站唯一的从站扫描端点，
 * 设备发现向导 / 设备扫描弹窗 / 单从站连通性探测都改调本文件，
 * 不再各自维护一套同端点的 HTTP 封装。
 */
export function respondedSlaves(results: SlaveScanResult[]): SlaveScanResult[] {
  return (results ?? []).filter((r) => r.responded)
}

/** 流式扫描事件回调（契约 p0-2 §4 事件序列：started → slave×N → summary/aborted/failed） */
export interface SlaveScanStreamHandlers {
  /** 租约建立后首发：携带计划探测地址数，用于初始化进度 */
  onStarted?: (info: SlaveScanStarted) => void
  /** 单地址探完（无论在线与否），地址升序；data 即 SlaveScanResult */
  onSlave?: (result: SlaveScanResult) => void
  /** 正常结束：计数 + 全量结果（可覆盖本地逐条累积） */
  onSummary?: (summary: SlaveScanSummary) => void
  /** 服务端主动停止（时长硬上限 reason=duration_cap），流随后关闭 */
  onAborted?: (info: { reason: string; scanned: number; total: number }) => void
  /** 总线中途掉线等致命错误，流随后关闭 */
  onFailed?: (info: { error: string; message: string; scanned: number; total: number }) => void
  /** 用户中断（AbortController）：后端在下一地址前停止，本函数正常 resolve 不 reject */
  signal?: AbortSignal
}

/** 当前环境是否可用流式扫描（不可用时调用方回落阻塞 scanBus） */
export function canStreamScan(): boolean {
  return canUseSseStream()
}

/**
 * 真实扫描·流式模式：逐地址推送探测结果，前端逐格点亮结果网格。
 * 传输层（鉴权头/解析/中断语义）见 sse.ts `postSseStream`；
 * 准入失败（missing_bus/bus_not_found/unsupported_protocol/bus_busy）在流建立前抛出中文 message。
 */
export async function scanBusStream(
  bus: string,
  rangeStart: number,
  rangeEnd: number,
  handlers: SlaveScanStreamHandlers,
): Promise<void> {
  await postSseStream(
    '/api/protocol-tools/slave-scan',
    { bus, range_start: rangeStart, range_end: rangeEnd },
    {
      signal: handlers.signal,
      onEvent: (event, json) => {
        switch (event) {
          case 'started': {
            const j = json as unknown as {
              bus: string; range_start: number; range_end: number; total: number
              timeout_ms: number; interval_ms: number; started_at_ms: number
            }
            handlers.onStarted?.({
              bus: j.bus, rangeStart: j.range_start, rangeEnd: j.range_end, total: j.total,
              timeoutMs: j.timeout_ms, intervalMs: j.interval_ms, startedAtMs: Number(j.started_at_ms),
            })
            break
          }
          case 'slave':
            handlers.onSlave?.(toSlaveScanResult(json as unknown as SlaveScanResultRaw))
            break
          case 'summary': {
            const j = json as unknown as {
              bus: string; scanned: number; found: number; noise: number
              duration_ms: number; results: SlaveScanResultRaw[]
            }
            handlers.onSummary?.({
              bus: j.bus, scanned: j.scanned, found: j.found, noise: j.noise,
              durationMs: Number(j.duration_ms), results: (j.results ?? []).map(toSlaveScanResult),
            })
            break
          }
          case 'aborted': {
            const j = json as unknown as { reason: string; scanned: number; total: number }
            handlers.onAborted?.({ reason: j.reason, scanned: j.scanned, total: j.total })
            break
          }
          case 'failed': {
            const j = json as unknown as { error: string; message: string; scanned: number; total: number }
            handlers.onFailed?.({ error: j.error, message: j.message, scanned: j.scanned, total: j.total })
            break
          }
        }
      },
    },
    '从站扫描请求失败',
  )
}
