/**
 * 设备运维 API（health / shadow / groups / assets）
 *
 * 路由：
 * - GET    /api/device-groups                  -> listGroups
 * - POST   /api/device-groups                  -> createGroup
 * - PUT    /api/device-groups/:id              -> updateGroup
 * - DELETE /api/device-groups/:id              -> deleteGroup
 * - POST   /api/device-groups/:id/devices      -> addDeviceToGroup
 * - DELETE /api/device-groups/:id/devices/:did -> removeDeviceFromGroup
 * - GET    /api/devices/assets                         -> listAssets
 * - GET    /api/devices/:id/asset                      -> getAsset
 * - PUT    /api/devices/:id/asset                      -> upsertAsset
 * - GET    /api/devices/health                         -> listHealth
 * - GET    /api/devices/shadows                        -> listShadows
 *
 * 第 1 期冗余收口：后端删除了 3 个与列表同源的重复端点
 * （`/api/devices/health/summary`、`/api/devices/:id/health`、
 * `/api/devices/:id/shadow`）。改为在列表内派生：
 * 汇总用 {@link summarizeHealth}，单设备快照在列表内 find。
 *
 * 后端实现：[../../../gateway-api/src/web/api/api_device_ops/]
 * 类型 [./generated/DeviceGroup|DeviceAsset|DeviceHealth|DeviceShadow] 由 ts-rs 生成。
 */
import { request } from './index'
import type { DeviceGroup, DeviceAsset, DeviceHealth, DeviceShadow } from './generated'

export type { DeviceGroup, DeviceAsset, DeviceHealth, DeviceShadow }

// ============================================================================
// 分组（持久化在 GatewayConfig.device_groups）
// ============================================================================

export interface GroupPayload {
  /** 仅 create 需要；update 不传 */
  id?: string
  name: string
  description: string
  color: string
  device_ids: string[]
}

// render 层统一用下面的 *View 适配函数；raw 版仅在文件内部使用，不对外导出
function listGroups(): Promise<DeviceGroup[]> {
  return request<DeviceGroup[]>({ url: '/api/device-groups', method: 'GET' })
}

function createGroup(payload: GroupPayload): Promise<DeviceGroup> {
  return request<DeviceGroup>({ url: '/api/device-groups', method: 'POST', data: payload })
}

function updateGroup(id: string, payload: Omit<GroupPayload, 'id'>): Promise<DeviceGroup> {
  return request<DeviceGroup>({
    url: `/api/device-groups/${encodeURIComponent(id)}`,
    method: 'PUT',
    data: payload,
  })
}

export function deleteGroup(id: string): Promise<void> {
  return request<void>({ url: `/api/device-groups/${encodeURIComponent(id)}`, method: 'DELETE' })
}

export function addDeviceToGroup(groupId: string, deviceId: string): Promise<void> {
  return request<void>({
    url: `/api/device-groups/${encodeURIComponent(groupId)}/devices`,
    method: 'POST',
    data: { device_id: deviceId },
  })
}

export function removeDeviceFromGroup(groupId: string, deviceId: string): Promise<void> {
  return request<void>({
    url: `/api/device-groups/${encodeURIComponent(groupId)}/devices/${encodeURIComponent(deviceId)}`,
    method: 'DELETE',
  })
}

// ============================================================================
// 资产标签（持久化在 GatewayConfig.device_assets）
// ============================================================================

export interface AssetPayload {
  device_id: string
  asset_no: string
  vendor: string
  model: string
  location: string
  owner: string
  purchased_at: string
  calibrated_at: string
  tags: string[]
}

function listAssets(): Promise<DeviceAsset[]> {
  return request<DeviceAsset[]>({ url: '/api/devices/assets', method: 'GET' })
}

function getAsset(deviceId: string): Promise<DeviceAsset> {
  return request<DeviceAsset>({ url: `/api/devices/${encodeURIComponent(deviceId)}/asset`, method: 'GET' })
}

function upsertAsset(payload: AssetPayload): Promise<DeviceAsset> {
  return request<DeviceAsset>({
    url: `/api/devices/${encodeURIComponent(payload.device_id)}/asset`,
    method: 'PUT',
    data: payload,
  })
}

// ============================================================================
// 健康评分（内存态，实时计算）
// ============================================================================

function listHealth(): Promise<DeviceHealth[]> {
  return request<DeviceHealth[]>({ url: '/api/devices/health', method: 'GET' })
}

// ============================================================================
// 设备影子（内存态，实时计算）
// ============================================================================

function listShadows(): Promise<DeviceShadow[]> {
  return request<DeviceShadow[]>({ url: '/api/devices/shadows', method: 'GET' })
}

// ==== VIEW-FRIENDLY 适配层 ====
// 后端 ts-rs 把 u64/i64 映射为 bigint，且字段名为 snake_case。
// View 历史用 camelCase + number 时间戳，这里统一适配，减少 view 改动。
// 调用方在 onMounted 中直接用 listGroupsView / listHealthView / listShadowsView / listAssetsView。

export interface DeviceGroupView {
  id: string
  name: string
  description: string
  color: string
  deviceIds: string[]
  createdAt: number
}

export interface DeviceHealthView {
  deviceId: string
  deviceName: string
  protocol: string
  bus: string
  /** null = 24h 内无采集，无法评分，UI 渲染「—」 */
  score: number | null
  level: 'excellent' | 'good' | 'warning' | 'critical' | 'unknown'
  successRate: number | null
  avgLatency: number | null
  errorCount: number
  offlineCount: number
  /** 好质量点位占比（%）；设备无点位时为 null */
  goodPointRatio: number | null
  /** 点位连续失败次数之和（当前快照） */
  consecutiveFailures: number
  /** 点位质量码分布（与数据质量页统计同源） */
  qualityBreakdown: { good: number; uncertain: number; bad: number; unknown: number; total: number }
  /** null = 从未成功采集 */
  lastSeenAt: number | null
  details: { label: string; value: string; score: number }[]
}

export interface DeviceShadowPointView {
  id: string
  quality: 'good' | 'uncertain' | 'bad'
  lastValue: string | null
}

export interface DeviceShadowView {
  deviceId: string
  deviceName: string
  protocol: string
  expectedIntervalMs: number
  lastValueHint: string | null
  lastQuality: string
  lastUpdateAt: number
  points: DeviceShadowPointView[]
}

export interface HealthSummaryView {
  total: number
  excellent: number
  good: number
  warning: number
  critical: number
  /** 无采集数据、无法评分的设备数 */
  unknown: number
  /** 有评分设备的平均分；全部无数据时为 null */
  avgScore: number | null
}

export interface DeviceAssetView {
  deviceId: string
  assetNo: string
  vendor: string
  model: string
  location: string
  owner: string
  purchasedAt: string
  calibratedAt: string
  tags: string[]
}

function mapLevel(s: string): DeviceHealthView['level'] {
  if (s === 'excellent') return 'excellent'
  if (s === 'good') return 'good'
  if (s === 'warning') return 'warning'
  if (s === 'critical') return 'critical'
  return 'unknown'
}

function mapQuality(q: string): DeviceShadowPointView['quality'] {
  if (q === 'good') return 'good'
  if (q === 'uncertain') return 'uncertain'
  return 'bad'
}

function toGroupView(g: DeviceGroup): DeviceGroupView {
  return {
    id: g.id,
    name: g.name,
    description: g.description,
    color: g.color,
    deviceIds: g.device_ids,
    createdAt: Number(g.created_at),
  }
}

function toHealthView(h: DeviceHealth): DeviceHealthView {
  return {
    deviceId: h.device_id,
    deviceName: h.device_name,
    protocol: h.protocol,
    bus: h.bus,
    score: h.score,
    level: mapLevel(h.level),
    successRate: h.success_rate,
    avgLatency: h.avg_latency_ms,
    errorCount: h.error_count,
    offlineCount: h.offline_count,
    goodPointRatio: h.good_point_ratio,
    consecutiveFailures: h.consecutive_failures,
    qualityBreakdown: h.quality_breakdown,
    lastSeenAt: h.last_seen_at == null ? null : Number(h.last_seen_at),
    details: h.details.map((d: { label: string; value: string; score: number }) => ({ label: d.label, value: d.value, score: d.score })),
  }
}

function toShadowView(s: DeviceShadow): DeviceShadowView {
  return {
    deviceId: s.device_id,
    deviceName: s.device_name,
    protocol: s.protocol,
    expectedIntervalMs: Number(s.expected_interval_ms),
    lastValueHint: s.last_value_hint,
    lastQuality: s.last_quality,
    lastUpdateAt: Number(s.last_update_at),
    points: s.points.map((p: { point_id: string; quality: string; last_value: string | null }) => ({
      id: p.point_id,
      quality: mapQuality(p.quality),
      lastValue: p.last_value,
    })),
  }
}

function toAssetView(a: DeviceAsset): DeviceAssetView {
  return {
    deviceId: a.device_id,
    assetNo: a.asset_no,
    vendor: a.vendor,
    model: a.model,
    location: a.location,
    owner: a.owner,
    purchasedAt: a.purchased_at,
    calibratedAt: a.calibrated_at,
    tags: a.tags,
  }
}

export function listGroupsView(): Promise<DeviceGroupView[]> {
  return listGroups().then((arr) => arr.map(toGroupView))
}

export function createGroupView(payload: GroupPayload): Promise<DeviceGroupView> {
  return createGroup(payload).then(toGroupView)
}

export function updateGroupView(id: string, payload: Omit<GroupPayload, 'id'>): Promise<DeviceGroupView> {
  return updateGroup(id, payload).then(toGroupView)
}

export function listHealthView(): Promise<DeviceHealthView[]> {
  return listHealth().then((arr) => arr.map(toHealthView))
}

/**
 * 从健康列表派生汇总（第 1 期冗余收口）。
 *
 * 原 `GET /api/devices/health/summary` 与 `/api/devices/health` 同源同判据
 * （同一份 `build_all`），后端已删除该端点；此处按同一口径在前端派生，
 * 保证「列表」与「汇总」永远一致，不会出现两次请求读到不同快照。
 */
export function summarizeHealth(rows: DeviceHealthView[]): HealthSummaryView {
  let excellent = 0
  let good = 0
  let warning = 0
  let critical = 0
  let unknown = 0
  let scoreSum = 0
  let scored = 0
  for (const r of rows) {
    if (r.level === 'excellent') excellent++
    else if (r.level === 'good') good++
    else if (r.level === 'warning') warning++
    else if (r.level === 'critical') critical++
    else unknown++
    if (r.score != null) {
      scoreSum += r.score
      scored++
    }
  }
  return {
    total: rows.length,
    excellent,
    good,
    warning,
    critical,
    unknown,
    avgScore: scored > 0 ? Math.round(scoreSum / scored) : null,
  }
}

/** 从健康列表中取单设备快照（替代已删除的 `GET /api/devices/:id/health`） */
export function findHealthView(rows: DeviceHealthView[], deviceId: string): DeviceHealthView | undefined {
  return rows.find((r) => r.deviceId === deviceId)
}

export function listShadowsView(): Promise<DeviceShadowView[]> {
  return listShadows().then((arr) => arr.map(toShadowView))
}

/** 从影子列表中取单设备快照（替代已删除的 `GET /api/devices/:id/shadow`） */
export function findShadowView(rows: DeviceShadowView[], deviceId: string): DeviceShadowView | undefined {
  return rows.find((r) => r.deviceId === deviceId)
}

export function listAssetsView(): Promise<DeviceAssetView[]> {
  return listAssets().then((arr) => arr.map(toAssetView))
}

export function getAssetView(deviceId: string): Promise<DeviceAssetView> {
  return getAsset(deviceId).then(toAssetView)
}

export function upsertAssetView(payload: AssetPayload): Promise<DeviceAssetView> {
  return upsertAsset(payload).then(toAssetView)
}

export const presetColors = [
  '#6366f1', '#67c23a', '#e6a23c', '#f56c6c', '#909399',
  '#9c27b0', '#00bcd4', '#ff5722', '#795548', '#3f51b5',
]

export const ALL_TAGS = ['电力', '关键', '温控', '水', '流量', 'PLC', '备用', '待维护', '已校准']

export function qualityLabel(q: DeviceShadowPointView['quality']): string {
  // 文案与全站质量口径统一（utils/qualityLabels）：good=良好、bad=坏值、uncertain=不确定
  return { good: '良好', uncertain: '不确定', bad: '坏值' }[q]
}

export function qualityColor(q: DeviceShadowPointView['quality']): string {
  // 调用方为 DeviceShadow 数值文字色（color）与质量指示点背景（background）。
  // 文字色必须用 --text-* 令牌：--color-success(#67c23a) 等原色在白底仅 2.04~2.90:1，
  // 不达 WCAG AA 4.5:1；--text-* 是同色系的深色变体（5.31~7.09:1），文字与小点均清晰。
  return {
    good: 'var(--text-success)',
    uncertain: 'var(--text-warning)',
    bad: 'var(--text-danger)',
  }[q]
}
