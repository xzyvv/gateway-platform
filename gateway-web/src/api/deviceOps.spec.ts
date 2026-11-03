import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  listGroupsView,
  createGroupView,
  updateGroupView,
  deleteGroup,
  addDeviceToGroup,
  removeDeviceFromGroup,
  listHealthView,
  summarizeHealth,
  findHealthView,
  listShadowsView,
  findShadowView,
  listAssetsView,
  getAssetView,
  upsertAssetView,
  qualityLabel,
  qualityColor,
  presetColors,
  ALL_TAGS,
} from './deviceOps'
import type { DeviceGroup, DeviceAsset, DeviceHealth, DeviceShadow } from './generated'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function groupRow(patch: Partial<DeviceGroup> = {}): DeviceGroup {
  return {
    id: 'g1',
    name: '一组',
    description: '描述',
    color: '#6366f1',
    device_ids: ['dev-1'],
    created_at: 1700000000000n,
    ...patch,
  }
}

function healthRow(patch: Partial<DeviceHealth> = {}): DeviceHealth {
  return {
    device_id: 'dev-1',
    device_name: '温度计',
    protocol: 'modbus',
    bus: 'bus1',
    score: 92,
    level: 'excellent',
    success_rate: 99.5,
    avg_latency_ms: 12,
    error_count: 1,
    offline_count: 0,
    good_point_ratio: 80,
    consecutive_failures: 0,
    quality_breakdown: { good: 4, uncertain: 0, bad: 1, unknown: 0, total: 5 },
    last_seen_at: 1700000000000,
    details: [{ label: '成功率', value: '99.5%', score: 99 }],
    ...patch,
  }
}

function shadowRow(patch: Partial<DeviceShadow> = {}): DeviceShadow {
  return {
    device_id: 'dev-1',
    device_name: '温度计',
    protocol: 'modbus',
    expected_interval_ms: 5000n,
    last_value_hint: '21.5',
    last_quality: 'good',
    last_update_at: 1700000000000n,
    points: [{ point_id: 'p1', quality: 'good', last_value: '21.5' }],
    ...patch,
  }
}

function assetRow(patch: Partial<DeviceAsset> = {}): DeviceAsset {
  return {
    device_id: 'dev-1',
    asset_no: 'A-001',
    vendor: '某厂商',
    model: 'M1',
    location: '1 号楼',
    owner: '张三',
    purchased_at: '2024-01-15',
    calibrated_at: '2024-06-01',
    tags: ['电力'],
    ...patch,
  }
}

describe('deviceOps 设备运维契约：分组', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定列表契约：分组集合走 GET /api/device-groups（不是 /api/devices/groups）
  it('分组列表走 GET /api/device-groups，并转换为 camelCase 视图', async () => {
    requestMock.mockResolvedValue([groupRow()])

    const rows = await listGroupsView()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/device-groups')
    // snake_case → camelCase + bigint 时间戳 → number
    expect(rows[0]).toEqual({
      id: 'g1',
      name: '一组',
      description: '描述',
      color: '#6366f1',
      deviceIds: ['dev-1'],
      createdAt: 1700000000000,
    })
  })

  // 锁定创建契约：POST 同路径，请求体原样透传（含可选 id）
  it('创建分组走 POST /api/device-groups，请求体原样透传', async () => {
    requestMock.mockResolvedValue(groupRow())

    await createGroupView({ id: 'g1', name: '一组', description: '描述', color: '#6366f1', device_ids: ['dev-1'] })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/device-groups')
    expect(cfg.data).toEqual({
      id: 'g1',
      name: '一组',
      description: '描述',
      color: '#6366f1',
      device_ids: ['dev-1'],
    })
  })

  // 锁定更新契约：PUT /api/device-groups/:id，请求体不含 id（id 只走路径）
  it('更新分组走 PUT /api/device-groups/:id，请求体不含 id', async () => {
    requestMock.mockResolvedValue(groupRow({ name: '新组名' }))

    await updateGroupView('g1', { name: '新组名', description: '描述', color: '#67c23a', device_ids: [] })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/device-groups/g1')
    expect(cfg.data).toEqual({ name: '新组名', description: '描述', color: '#67c23a', device_ids: [] })
    expect((cfg.data as Record<string, unknown>).id).toBeUndefined()
  })

  // 锁定 id 编码契约：含特殊字符的分组 id 必须 URL 编码，避免拼出非法路径
  it('分组 id 做 URL 编码', async () => {
    requestMock.mockResolvedValue(groupRow())

    await updateGroupView('a/b', { name: 'x', description: '', color: '#000', device_ids: [] })

    expect(firstCallConfig().url).toBe('/api/device-groups/a%2Fb')
  })

  // 锁定删除契约：DELETE /api/device-groups/:id
  it('删除分组走 DELETE /api/device-groups/:id', async () => {
    requestMock.mockResolvedValue(undefined)

    await deleteGroup('g1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/device-groups/g1')
  })

  // 锁定成员增删契约：POST .../devices 带 { device_id }；DELETE .../devices/:did 不带 body
  it('添加/移除分组成员：POST 与 DELETE 的路径与请求体形状', async () => {
    requestMock.mockResolvedValue(undefined)

    await addDeviceToGroup('g1', 'dev-1')
    let cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/device-groups/g1/devices')
    expect(cfg.data).toEqual({ device_id: 'dev-1' })

    requestMock.mockReset()
    requestMock.mockResolvedValue(undefined)
    await removeDeviceFromGroup('g1', 'dev-1')
    cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/device-groups/g1/devices/dev-1')
    expect(cfg.data).toBeUndefined()
  })
})

describe('deviceOps 设备运维契约：健康分', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定健康列表契约：GET /api/devices/health
  it('健康列表走 GET /api/devices/health，字段转换为 camelCase', async () => {
    requestMock.mockResolvedValue([healthRow()])

    const rows = await listHealthView()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/health')
    expect(rows[0]).toMatchObject({
      deviceId: 'dev-1',
      deviceName: '温度计',
      protocol: 'modbus',
      bus: 'bus1',
      score: 92,
      level: 'excellent',
      successRate: 99.5,
      avgLatency: 12,
      errorCount: 1,
      offlineCount: 0,
      goodPointRatio: 80,
      consecutiveFailures: 0,
      qualityBreakdown: { good: 4, uncertain: 0, bad: 1, unknown: 0, total: 5 },
      lastSeenAt: 1700000000000,
    })
    expect(rows[0]!.details).toEqual([{ label: '成功率', value: '99.5%', score: 99 }])
  })

  // 锁定 level 枚举契约：后端未知 level 统一回落为 unknown，不透传脏值
  it('未知 level 归一化为 unknown', async () => {
    requestMock.mockResolvedValue([healthRow({ level: 'weird' })])

    const rows = await listHealthView()

    expect(rows[0]!.level).toBe('unknown')
  })

  // 锁定无采集设备的边界：score/lastSeenAt 为 null 时原样保留 null（UI 渲染「—」）
  it('无采集设备 score / lastSeenAt 保留 null', async () => {
    requestMock.mockResolvedValue([healthRow({ score: null, last_seen_at: null as unknown as number })])

    const rows = await listHealthView()

    expect(rows[0]!.score).toBeNull()
    expect(rows[0]!.lastSeenAt).toBeNull()
  })

  // 第 1 期冗余收口：`GET /api/devices/health/summary` 已删除，汇总由列表派生。
  // 这里锁定派生口径：等级计数 + 有评分设备的平均分（全部无评分时为 null）。
  it('健康汇总由列表派生：等级计数与平均分', async () => {
    requestMock.mockResolvedValue([
      healthRow({ device_id: 'dev-1', score: 92, level: 'excellent' }),
      healthRow({ device_id: 'dev-2', score: 84, level: 'good' }),
      healthRow({ device_id: 'dev-3', score: null, level: 'unknown' }),
    ])

    const rows = await listHealthView()
    const view = summarizeHealth(rows)

    expect(view).toEqual({
      total: 3,
      excellent: 1,
      good: 1,
      warning: 0,
      critical: 0,
      unknown: 1,
      avgScore: 88,
    })
    // 汇总不再发起第二个请求（列表与汇总同一份快照）
    expect(requestMock).toHaveBeenCalledTimes(1)
  })

  // 全部无评分时平均分必须为 null，不得用 0 分冒充
  it('健康汇总在全部无评分时 avgScore 为 null', async () => {
    requestMock.mockResolvedValue([
      healthRow({ device_id: 'dev-1', score: null, level: 'unknown' }),
    ])

    const rows = await listHealthView()

    expect(summarizeHealth(rows).avgScore).toBeNull()
  })

  // 第 1 期冗余收口：`GET /api/devices/:id/health` 已删除，单设备快照在列表内 find
  it('单设备健康快照由列表派生，不再请求单设备端点', async () => {
    requestMock.mockResolvedValue([
      healthRow({ device_id: 'dev-1' }),
      healthRow({ device_id: 'dev-2', device_name: '压力表' }),
    ])

    const rows = await listHealthView()
    const found = findHealthView(rows, 'dev-2')

    expect(found?.deviceName).toBe('压力表')
    expect(findHealthView(rows, 'dev-404')).toBeUndefined()
    expect(requestMock).toHaveBeenCalledTimes(1)
  })
})

describe('deviceOps 设备运维契约：设备影子', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定影子列表契约：GET /api/devices/shadows（复数 shadows）
  it('影子列表走 GET /api/devices/shadows，bigint 时间戳转 number', async () => {
    requestMock.mockResolvedValue([shadowRow()])

    const rows = await listShadowsView()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/shadows')
    expect(rows[0]).toEqual({
      deviceId: 'dev-1',
      deviceName: '温度计',
      protocol: 'modbus',
      expectedIntervalMs: 5000,
      lastValueHint: '21.5',
      lastQuality: 'good',
      lastUpdateAt: 1700000000000,
      points: [{ id: 'p1', quality: 'good', lastValue: '21.5' }],
    })
  })

  // 锁定点位质量码契约：good/uncertain 原样，其余一律归为 bad
  it('点位质量码仅保留 good/uncertain，其余归为 bad', async () => {
    requestMock.mockResolvedValue([
      shadowRow({
        points: [
          { point_id: 'p1', quality: 'good', last_value: '1' },
          { point_id: 'p2', quality: 'uncertain', last_value: null },
          { point_id: 'p3', quality: 'stale', last_value: null },
        ],
      }),
    ])

    const rows = await listShadowsView()

    expect(rows[0]!.points.map((p) => p.quality)).toEqual(['good', 'uncertain', 'bad'])
    expect(rows[0]!.points[1]!.lastValue).toBeNull()
  })

  // 第 1 期冗余收口：`GET /api/devices/:id/shadow` 已删除，单设备快照在列表内 find
  it('单设备影子快照由列表派生，不再请求单设备端点', async () => {
    requestMock.mockResolvedValue([
      shadowRow({ device_id: 'dev-1' }),
      shadowRow({ device_id: 'dev-2', device_name: '压力表' }),
    ])

    const rows = await listShadowsView()
    const found = findShadowView(rows, 'dev-2')

    expect(found?.deviceName).toBe('压力表')
    expect(findShadowView(rows, 'dev-404')).toBeUndefined()
    expect(requestMock).toHaveBeenCalledTimes(1)
  })
})

describe('deviceOps 设备运维契约：资产标签', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定资产列表契约：GET /api/devices/assets（与 /api/devices/:id/asset 区分）
  it('资产列表走 GET /api/devices/assets，字段转换为 camelCase', async () => {
    requestMock.mockResolvedValue([assetRow()])

    const rows = await listAssetsView()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/assets')
    expect(rows[0]).toEqual({
      deviceId: 'dev-1',
      assetNo: 'A-001',
      vendor: '某厂商',
      model: 'M1',
      location: '1 号楼',
      owner: '张三',
      purchasedAt: '2024-01-15',
      calibratedAt: '2024-06-01',
      tags: ['电力'],
    })
  })

  // 锁定单设备资产契约：GET /api/devices/:id/asset
  it('单设备资产走 GET /api/devices/:id/asset', async () => {
    requestMock.mockResolvedValue(assetRow())

    await getAssetView('dev-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/dev-1/asset')
  })

  // 锁定 upsert 契约：PUT /api/devices/:id/asset，device_id 取自 payload 且进路径
  it('资产 upsert 走 PUT /api/devices/:id/asset，device_id 取自 payload', async () => {
    const payload = {
      device_id: 'dev-9',
      asset_no: 'A-009',
      vendor: 'v',
      model: 'm',
      location: 'l',
      owner: 'o',
      purchased_at: '2025-01-01',
      calibrated_at: '2025-02-01',
      tags: [],
    }
    requestMock.mockResolvedValue(assetRow({ device_id: 'dev-9', asset_no: 'A-009' }))

    await upsertAssetView(payload)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/devices/dev-9/asset')
    expect(cfg.data).toEqual(payload)
  })
})

describe('deviceOps 质量文案与常量', () => {
  it('qualityLabel 与全站口径一致（良好/坏值/不确定）', () => {
    expect(qualityLabel('good')).toBe('良好')
    expect(qualityLabel('uncertain')).toBe('不确定')
    expect(qualityLabel('bad')).toBe('坏值')
  })

  // 锁定颜色令牌契约：必须用 --text-* 深色令牌（白底可达 WCAG AA），不是 --color-* 原色
  it('qualityColor 返回 --text-* 令牌', () => {
    expect(qualityColor('good')).toBe('var(--text-success)')
    expect(qualityColor('uncertain')).toBe('var(--text-warning)')
    expect(qualityColor('bad')).toBe('var(--text-danger)')
  })

  it('预置颜色与标签常量非空且格式正确', () => {
    expect(presetColors.length).toBeGreaterThan(0)
    presetColors.forEach((c) => expect(c).toMatch(/^#[0-9a-fA-F]{6}$/))
    expect(ALL_TAGS).toContain('电力')
  })
})
