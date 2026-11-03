/**
 * 读点即时读值（E1：读点语义唯一实现）
 *
 * 背景：`GET /api/points/:id/read` 在采集失败时仍返回 200 + `{ status: 'error', error }`。
 * 之前 7 个调用点各自判断：有的只看 `value == null`，有的只看 promise 是否 reject，
 * 于是「后端已明确返回 status=error」的点位在连通性检测里会被算作在线。
 *
 * 本 composable 把「一次读点」收敛成统一的 `PointReadOutcome`：
 *   - `ok` 只在 `status === 'ok'` 且拿到有效值时为 true；
 *   - 协议错误（200 + status=error）与传输错误（reject）都落到 `ok=false` + `error` 文本；
 *   - 延迟由同一次调用测量，避免各页面各自 `performance.now()` 口径不一。
 *
 * 各页面只负责「把 outcome 映射成自己的 UI」，不再各自解释后端语义。
 */
import { readPointNow } from '@/api/points'
import { extractErrorMessage } from '@/utils/errorHandler'

/** 单次读点的统一结果，不区分协议错误 / 传输错误 */
export interface PointReadOutcome {
  pointId: string
  /** 是否读到有效值（status=ok 且有值） */
  ok: boolean
  /** 有效值；失败时为 null */
  value: number | null
  /** 后端原样状态词：ok / error / 空串（传输失败时为空） */
  status: string
  /** 失败原因（协议错误原文或传输错误信息），成功时为空串 */
  error: string
  /** 后端质量码（原样透出，供追踪页等需要完整响应的场景使用） */
  quality: unknown
  /** 本次读点耗时（毫秒） */
  latencyMs: number
  /** 原始响应；传输失败时为 null */
  raw: { status: string; value?: number; error?: string; quality?: unknown } | null
}

export interface ReadManyOptions {
  /** 每完成一条回调一次，便于页面逐条追加日志 */
  onResult?: (outcome: PointReadOutcome) => void
  /** 返回 true 时停止取后续点位（组件卸载 / 用户取消） */
  shouldStop?: () => boolean
}

function normalize(
  pointId: string,
  raw: { status: string; value?: number; error?: string; quality?: unknown },
  latencyMs: number,
): PointReadOutcome {
  const status = String(raw?.status ?? '')
  const value = raw?.value ?? null
  const ok = status === 'ok' && value !== null
  return {
    pointId,
    ok,
    value: ok ? value : null,
    status,
    error: ok ? '' : raw?.error || (status ? `采集返回 status=${status}` : '读取失败'),
    quality: raw?.quality,
    latencyMs,
    raw,
  }
}

function transportFailure(pointId: string, e: unknown, latencyMs: number): PointReadOutcome {
  return {
    pointId,
    ok: false,
    value: null,
    status: '',
    error: extractErrorMessage(e, '读取失败'),
    quality: undefined,
    latencyMs,
    raw: null,
  }
}

export function useReadPointNow() {
  /** 读一次点位；无论协议失败还是请求失败都不抛异常，统一返回 outcome */
  async function readOnce(pointId: string): Promise<PointReadOutcome> {
    const t0 = performance.now()
    try {
      const raw = await readPointNow(pointId)
      return normalize(pointId, raw, performance.now() - t0)
    } catch (e) {
      return transportFailure(pointId, e, performance.now() - t0)
    }
  }

  /** 按给定顺序逐个读取；任一失败不影响后续 */
  async function readSequential(
    pointIds: string[],
    options: ReadManyOptions = {},
  ): Promise<PointReadOutcome[]> {
    const results: PointReadOutcome[] = []
    for (const id of pointIds) {
      if (options.shouldStop?.()) break
      const outcome = await readOnce(id)
      results.push(outcome)
      options.onResult?.(outcome)
    }
    return results
  }

  return { readOnce, readSequential }
}
