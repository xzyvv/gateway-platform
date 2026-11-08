/**
 * 点位运维 API（virtual / find-replace / offset / clone）
 *
 * 路由：
 * - GET    /api/point-tools/virtual                  -> listVirtualPoints
 * - POST   /api/point-tools/virtual                  -> createVirtualPoint
 * - PUT    /api/point-tools/virtual/:id              -> updateVirtualPoint
 * - DELETE /api/point-tools/virtual/:id              -> deleteVirtualPoint
 * - POST   /api/point-tools/virtual/:id/recalc       -> recalcVirtualPoint
 * - POST   /api/point-tools/find-replace/search      -> findReplaceSearch
 * - POST   /api/point-tools/find-replace/apply       -> findReplaceApply
 * - GET    /api/point-tools/offset                    -> offsetList
 * - POST   /api/point-tools/offset/apply              -> offsetApply
 * - GET    /api/point-tools/clone/devices             -> cloneDevices
 * - POST   /api/point-tools/clone/apply               -> cloneApply
 *
 * 后端实现：[../../../gateway-api/src/web/api/api_points_ops/]
 * 类型由 ts-rs 生成。
 */
import { request } from './index'
import type { VirtualPointView, SearchResult, OffsetPointView, CloneDevice } from './generated'

export type { VirtualPointView as VirtualPoint, SearchResult, OffsetPointView, CloneDevice }

// ============================================================================
// 虚拟点位（持久化在 GatewayConfig.virtual_points）
// ============================================================================

export interface VirtualPointPayload {
  id?: string
  name: string
  device_id?: string
  formula: string
  unit?: string
  decimals?: number
  enabled?: boolean
}

// render 层统一用下面的 *View 适配函数；raw 版仅在文件内部使用（deleteVirtualPoint 例外，供页面直接调用）
function listVirtualPoints(): Promise<VirtualPointView[]> {
  return request<VirtualPointView[]>({ url: '/api/point-tools/virtual', method: 'GET' })
}

function createVirtualPoint(payload: VirtualPointPayload): Promise<VirtualPointView> {
  return request<VirtualPointView>({ url: '/api/point-tools/virtual', method: 'POST', data: payload })
}

function updateVirtualPoint(id: string, payload: VirtualPointPayload): Promise<VirtualPointView> {
  return request<VirtualPointView>({
    url: `/api/point-tools/virtual/${encodeURIComponent(id)}`,
    method: 'PUT',
    data: payload,
  })
}

export function deleteVirtualPoint(id: string): Promise<void> {
  return request<void>({
    url: `/api/point-tools/virtual/${encodeURIComponent(id)}`,
    method: 'DELETE',
  })
}

function recalcVirtualPoint(id: string): Promise<VirtualPointView> {
  return request<VirtualPointView>({
    url: `/api/point-tools/virtual/${encodeURIComponent(id)}/recalc`,
    method: 'POST',
  })
}

// ============================================================================
// 批量查找替换（直接操作 PointConfig，热重载）
// ============================================================================

export interface FindReplaceSearchRequest {
  keyword: string
  /** 'name' | 'address' | 'unit' | 'all' */
  scope?: string
}

export interface FindReplaceApplyRequest {
  ids: string[]
  /** 'name' | 'address' | 'unit' */
  field: string
  from: string
  to: string
}

function findReplaceSearch(payload: FindReplaceSearchRequest): Promise<SearchResult[]> {
  return request<SearchResult[]>({
    url: '/api/point-tools/find-replace/search',
    method: 'POST',
    data: payload,
  })
}

function findReplaceApply(payload: FindReplaceApplyRequest): Promise<{ updated: number }> {
  return request<{ updated: number }>({
    url: '/api/point-tools/find-replace/apply',
    method: 'POST',
    data: payload,
  })
}

// ============================================================================
// 批量地址偏移
// ============================================================================

function offsetList(): Promise<OffsetPointView[]> {
  return request<OffsetPointView[]>({ url: '/api/point-tools/offset', method: 'GET' })
}

function offsetApply(ids: string[], offset: number): Promise<{ updated: number }> {
  return request<{ updated: number }>({
    url: '/api/point-tools/offset/apply',
    method: 'POST',
    data: { ids, offset },
  })
}

// ============================================================================
// 批量点位克隆
// ============================================================================

// D-02：源/目标设备名单是同一份数据（后端两份实现逐行等价），已合并为一个端点
function cloneDevices(): Promise<CloneDevice[]> {
  return request<CloneDevice[]>({ url: '/api/point-tools/clone/devices', method: 'GET' })
}

function cloneApply(
  sourceDeviceId: string,
  targetDeviceIds: string[],
  nameSuffix = '',
): Promise<{ total: number; targets: string[] }> {
  return request<{ total: number; targets: string[] }>({
    url: '/api/point-tools/clone/apply',
    method: 'POST',
    data: {
      source_device_id: sourceDeviceId,
      target_device_ids: targetDeviceIds,
      name_suffix: nameSuffix,
    },
  })
}

// ==== VIEW-FRIENDLY 适配层 ====
// 后端字段 snake_case，view 历史 camelCase；这里统一映射 + 把 selected/newValue 等 UI 态字段加进来

export interface VirtualPointViewV {
  id: string
  name: string
  deviceId: string
  formula: string
  unit: string
  decimals: number
  enabled: boolean
  lastValue: number | null
  error: string | null
  createdAt: number
}

export interface SearchResultView {
  id: string
  field: string
  deviceName: string
  pointName: string
  oldValue: string
  newValue: string | null
  selected: boolean
}

export interface OffsetPointViewV {
  id: string
  name: string
  oldAddr: number
  newAddr: number
  type: string
  selected: boolean
}

export interface CloneSourceView {
  id: string
  name: string
  pointCount: number
  // 源设备点位预览（V1：暂用空数组，后端未单独提供；前端可调 listPoints(deviceId=...)）
  points: { id: string; name: string; addr: number; type: string }[]
}

export interface CloneTargetView {
  deviceId: string
  deviceName: string
  selected: boolean
  pointCount: number
}

function toVirtualPointViewV(v: VirtualPointView): VirtualPointViewV {
  return {
    id: v.id,
    name: v.name,
    deviceId: v.device_id,
    formula: v.formula,
    unit: v.unit,
    decimals: v.decimals,
    enabled: v.enabled,
    lastValue: v.last_value,
    error: v.error,
    createdAt: v.created_at,
  }
}

function toSearchResultView(s: SearchResult): SearchResultView {
  return {
    id: s.id,
    field: s.field,
    deviceName: s.device_name,
    pointName: s.point_name,
    oldValue: s.old_value,
    newValue: null,
    selected: false,
  }
}

function toOffsetPointViewV(o: OffsetPointView): OffsetPointViewV {
  return {
    id: o.id,
    name: o.name,
    oldAddr: o.old_addr,
    newAddr: o.new_addr,
    type: o.data_type,
    selected: o.selected,
  }
}

function toCloneSourceView(s: CloneDevice): CloneSourceView {
  return {
    id: s.id,
    name: s.name,
    pointCount: s.point_count,
    points: [],
  }
}

function toCloneTargetView(t: CloneDevice): CloneTargetView {
  return {
    deviceId: t.id,
    deviceName: t.name,
    selected: false,
    pointCount: t.point_count,
  }
}

export function listVirtualPointsView(): Promise<VirtualPointViewV[]> {
  return listVirtualPoints().then((arr) => arr.map(toVirtualPointViewV))
}

export function createVirtualPointView(payload: VirtualPointPayload): Promise<VirtualPointViewV> {
  return createVirtualPoint(payload).then(toVirtualPointViewV)
}

export function updateVirtualPointView(id: string, payload: VirtualPointPayload): Promise<VirtualPointViewV> {
  return updateVirtualPoint(id, payload).then(toVirtualPointViewV)
}

export function recalcVirtualPointView(id: string): Promise<{ value?: number; error?: string }> {
  return recalcVirtualPoint(id).then((v) => ({
    value: v.last_value ?? undefined,
    error: v.error ?? undefined,
  }))
}

export function findReplaceSearchView(keyword: string, scope: string): Promise<SearchResultView[]> {
  return findReplaceSearch({ keyword, scope }).then((arr) => arr.map(toSearchResultView))
}

export function findReplaceApplyView(
  targets: SearchResultView[],
  field: string,
  from: string,
  to: string,
): Promise<{ count: number }> {
  const ids = targets.filter((t) => t.selected).map((t) => t.id)
  return findReplaceApply({ ids, field, from, to }).then((r) => ({ count: r.updated }))
}

export function listOffsetPointsView(): Promise<OffsetPointViewV[]> {
  return offsetList().then((arr) => arr.map(toOffsetPointViewV))
}

export function applyOffsetView(
  points: OffsetPointViewV[],
  offset: number,
): Promise<{ updated: number }> {
  const ids = points.filter((p) => p.selected).map((p) => p.id)
  return offsetApply(ids, offset)
}

export function listCloneSourcesView(): Promise<CloneSourceView[]> {
  return cloneDevices().then((arr) => arr.map(toCloneSourceView))
}

export function listCloneTargetsView(): Promise<CloneTargetView[]> {
  return cloneDevices().then((arr) => arr.map(toCloneTargetView))
}

export function cloneApplyView(
  sourceId: string,
  targets: CloneTargetView[],
  nameSuffix: string,
): Promise<{ total: number; targets: string[] }> {
  const ids = targets.filter((t) => t.selected).map((t) => t.deviceId)
  return cloneApply(sourceId, ids, nameSuffix)
}
