import { request } from './index'
import type { PointConfig } from './types'

/**
 * 点位新增/编辑请求体，与后端 `PointPayload` 对齐。
 *
 * Bug W4: 后端 `PointPayload.interval` 是必填 `u64`（且校验 1..=59），而前端此前只发
 * 7 个字段并用 `as PointConfig` 强断言掩盖缺失，后端 serde 反序列化直接 422
 * （missing field `interval`），导致新增/编辑点位必然失败。
 * 这里显式声明必填字段，去掉强断言，让 TS 在编译期拦住契约不一致。
 */
export type PointPayload = Partial<PointConfig> & {
  id: string
  bus: string
  protocol: string
  address: string
  interval: number
}

/** 获取指定设备的点位列表 */
export function listPoints(deviceId: string): Promise<PointConfig[]> {
  return request<PointConfig[]>({ url: `/api/devices/${deviceId}/points`, method: 'GET' })
}

/** 创建点位 */
export function createPoint(deviceId: string, point: PointPayload): Promise<PointConfig> {
  return request<PointConfig>({
    url: `/api/devices/${deviceId}/points`,
    method: 'POST',
    data: point,
  })
}

/** 更新点位 */
export function updatePoint(id: string, point: PointPayload): Promise<PointConfig> {
  return request<PointConfig>({ url: `/api/points/${id}`, method: 'PUT', data: point })
}

/** 删除点位（软删除，进入回收站） */
export function deletePoint(id: string): Promise<{ status: string }> {
  return request({ url: `/api/points/${id}`, method: 'DELETE' })
}

/**
 * 回收站条目：被软删除的点位快照 + 删除时间（毫秒时间戳）。
 *
 * 该结构仅用于 `/api/point-trash` 的响应包装，不是后端 ts-rs 导出的领域类型，
 * 因此定义在本模块而非 `api/types.ts`（后者只放生成类型与全局共享类型）。
 */
export interface PointTrashEntry {
  point: PointConfig
  deleted_at_ms: number
}

/**
 * 获取回收站点位列表（按 deleted_at_ms 倒序）。
 *
 * 注意前缀是 `/api/point-trash` 而不是 `/api/points/trash`：后者会与
 * `DELETE /api/points/:id` 的静态段遮蔽（id 恰好为 `trash` 的点位会被路由到
 * 「清空回收站」）。改动前先看后端 `api/points/routes.rs` 的说明。
 */
export function listPointTrash(): Promise<PointTrashEntry[]> {
  return request<PointTrashEntry[]>({ url: '/api/point-trash', method: 'GET' })
}

/**
 * 从回收站恢复单个点位。
 *
 * 后端会校验 id 唯一性：若同 id 点位已存在（例如删除后又新建了同 id 点位），
 * 恢复会失败并由全局拦截器留痕，调用方需自行给出「到回收站重试/改名」提示。
 */
export function restorePoint(id: string): Promise<null> {
  return request<null>({ url: `/api/points/${id}/restore`, method: 'POST' })
}

/** 彻底删除回收站中的单个点位（不可恢复） */
export function purgeTrashPoint(id: string): Promise<null> {
  return request<null>({ url: `/api/point-trash/${id}`, method: 'DELETE' })
}

/** 清空回收站（不可恢复） */
export function purgeAllTrash(): Promise<null> {
  return request<null>({ url: '/api/point-trash', method: 'DELETE' })
}

/** 批量导入点位 */
export function batchImportPoints(points: PointConfig[]): Promise<{ status: string }> {
  return request({ url: '/api/point-tools/batch', method: 'POST', data: points })
}

/**
 * 批量编辑点位（增/改/删）。
 *
 * R3：后端已把 `/api/point-tools/batch/edit` 合并进 `/api/point-tools/batch`，
 * 通过 `action: 'edit'` 判别分支，故此处不再单独请求旧端点。
 */
export function batchEditPoints(data: {
  add: PointConfig[]
  update: PointConfig[]
  delete: string[]
}): Promise<{ status: string }> {
  return request({
    url: '/api/point-tools/batch',
    method: 'POST',
    data: { action: 'edit', ...data },
  })
}

/**
 * 手动读取点位
 *
 * 后端失败时仍返回 200 + `{ status: 'error', error, quality }`，
 * 因此 `error` 必须参与类型，调用方才不会把失败当成空值吞掉。
 */
export function readPointNow(id: string): Promise<{ status: string; value?: number; error?: string; quality?: unknown }> {
  return request({ url: `/api/points/${id}/read`, method: 'GET' })
}

/** 统一写入口的单条：点位 id + 目标值（与后端 `PointWriteEntry` 对齐） */
export interface PointWriteEntry {
  id: string
  value: number
}

/**
 * 写入单项结果，与后端统一写入口 `POST /api/point-tools/write` 的 `results` 元素对齐。
 * `results` 与入参**等长且逐位对应**；单点失败不影响其余点位。
 */
export interface BatchWriteResultItem {
  point_id: string
  status: string
  value?: number
  error?: string
  quality?: unknown
}

/** 统一写响应：status 为 ok | partial | error */
export interface WritePointsResponse {
  status: string
  total: number
  succeeded: number
  failed: number
  results: (BatchWriteResultItem | null)[]
}

/**
 * 点位写入统一入口（B2）
 *
 * `POST /api/point-tools/write`，请求体 `{ writes: [{ id, value }] }`。
 * 前端只传数组，单/批分发由后端决定：
 * - 1 条走单写业务（查点 / writable 白名单 / 审计与远程控制完全一致；
 *   写失败返回业务码 5101，由 axios 拦截器 reject，调用方走错误分支）；
 * - ≥2 条走批写业务（按总线分组 `write_multi`，单条失败在 `results` 对应位置返回）。
 *
 * 三个写入口页（点位读写测试、远程控制、批量命令）统一调用本函数。
 */
export function writePoints(writes: PointWriteEntry[]): Promise<WritePointsResponse> {
  return request({ url: '/api/point-tools/write', method: 'POST', data: { writes } })
}
