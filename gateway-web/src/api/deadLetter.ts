/**
 * 设备端死信队列 API（Webhook 上报永久失败的数据批次）
 *
 * 与 `composables/uplink/useWebhook` 的「通知重试队列」无关：
 * 那边是**本机浏览器**缓存的通知重发项，这里是**网关设备端**持久化的未送达数据。
 *
 * 契约：
 * - GET    /api/north/dead-letters?protocol=http&limit=50&offset=0
 * - POST   /api/north/dead-letters/{id}/replay
 * - DELETE /api/north/dead-letters?protocol=http[&older_than_ms=<epoch_ms>]
 *
 * 三者不可用时后端返回 HTTP 503（code=5003）。前端必须把 503 与「0 条」区分开：
 * 503 表示「查不到，无法确认有没有未送达数据」，绝不能渲染成 0 条。
 */
import { request } from './index'
import type { DeadLetterDto } from './generated/DeadLetterDto'

/** 北向协议名：后端缺省已是 http（WebhookClient 落库用的就是它），这里显式传避免依赖默认值 */
export const DEAD_LETTER_PROTOCOL = 'http'

/**
 * 单条死信记录。
 *
 * D3：直接复用后端 ts-rs 生成的 `DeadLetterDto`，不再手抄一份 interface。
 * 手抄件与后端字段漂移时（改名/加字段）前端不会编译报错，只会在运行时静默
 * 拿到 `undefined`；生成类型能把这种漂移变成编译错误。字段含义见后端
 * `gateway-api/src/web/traits.rs` 的 `DeadLetterDto`。
 */
export type DeadLetterItem = DeadLetterDto

export interface DeadLetterList {
  protocol: string
  /** 该协议死信总数（不受 limit/offset 影响） */
  total: number
  items: DeadLetterItem[]
  /** 因超过死信表行数上限被自动裁剪（永久删除）的累计条数 */
  trimmed_total: number
  /** 最近一次自动裁剪的时间（epoch 毫秒），0 = 从未裁剪 */
  last_trim_ms: number
  /** 最近一次自动裁剪删除的条数 */
  last_trim_rows: number
}

export interface DeadLetterReplayResult {
  /** 死信记录 id */
  id: number
  /** 重投后生成的新 outbox 行 id；0 表示同一 data_id 已有在途记录 */
  outbox_id: number
  /** 是否真正重新入队 */
  requeued: boolean
}

export interface DeadLetterPurgeResult {
  /** 实际删除条数 */
  deleted: number
}

/** 查询设备端死信列表（payload 仅预览） */
export function listDeadLetters(opts: { limit?: number; offset?: number } = {}): Promise<DeadLetterList> {
  const limit = opts.limit ?? 50
  const offset = opts.offset ?? 0
  return request<DeadLetterList>({
    url: `/api/north/dead-letters?protocol=${DEAD_LETTER_PROTOCOL}&limit=${limit}&offset=${offset}`,
    method: 'GET',
  })
}

/** 重投单条死信：修好对端后把数据重新交给北向通道发送 */
export function replayDeadLetter(id: number): Promise<DeadLetterReplayResult> {
  return request<DeadLetterReplayResult>({
    url: `/api/north/dead-letters/${id}/replay`,
    method: 'POST',
  })
}

/**
 * 清理设备端死信（不可逆：死信表是这批数据唯一的副本）。
 * 不传 olderThanMs 表示清理该协议全部死信。
 */
export function purgeDeadLetters(olderThanMs?: number): Promise<DeadLetterPurgeResult> {
  const params = new URLSearchParams({ protocol: DEAD_LETTER_PROTOCOL })
  if (olderThanMs != null) params.set('older_than_ms', String(olderThanMs))
  return request<DeadLetterPurgeResult>({
    url: `/api/north/dead-letters?${params.toString()}`,
    method: 'DELETE',
  })
}
