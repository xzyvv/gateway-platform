import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  listVirtualPointsView,
  createVirtualPointView,
  updateVirtualPointView,
  deleteVirtualPoint,
  recalcVirtualPointView,
  findReplaceSearchView,
  findReplaceApplyView,
  listOffsetPointsView,
  applyOffsetView,
  listCloneSourcesView,
  listCloneTargetsView,
  cloneApplyView,
} from './pointsOps'
import type { VirtualPointView, SearchResult, OffsetPointView, CloneDevice } from './generated'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function virtualRow(patch: Partial<VirtualPointView> = {}): VirtualPointView {
  return {
    id: 'vp1',
    name: '虚拟压力',
    device_id: 'dev-1',
    formula: 'a + b',
    unit: 'kPa',
    decimals: 2,
    enabled: true,
    last_value: 12.5,
    error: null,
    created_at: 1700000000000,
    ...patch,
  }
}

describe('pointsOps 虚拟点位契约', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定列表契约：GET /api/points/virtual
  it('虚拟点位列表走 GET /api/points/virtual，字段转换为 camelCase', async () => {
    requestMock.mockResolvedValue([virtualRow()])

    const rows = await listVirtualPointsView()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/points/virtual')
    expect(rows[0]).toEqual({
      id: 'vp1',
      name: '虚拟压力',
      deviceId: 'dev-1',
      formula: 'a + b',
      unit: 'kPa',
      decimals: 2,
      enabled: true,
      lastValue: 12.5,
      error: null,
      createdAt: 1700000000000,
    })
  })

  // 锁定创建契约：POST 同路径 + 请求体原样透传
  it('创建虚拟点位走 POST /api/points/virtual，请求体原样透传', async () => {
    requestMock.mockResolvedValue(virtualRow())

    await createVirtualPointView({ id: 'vp1', name: '虚拟压力', device_id: 'dev-1', formula: 'a + b' })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/virtual')
    expect(cfg.data).toEqual({ id: 'vp1', name: '虚拟压力', device_id: 'dev-1', formula: 'a + b' })
  })

  // 锁定更新契约：PUT /api/points/virtual/:id（id 进路径）
  it('更新虚拟点位走 PUT /api/points/virtual/:id', async () => {
    requestMock.mockResolvedValue(virtualRow({ name: '新名' }))

    await updateVirtualPointView('vp1', { name: '新名', formula: 'a + b' })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/points/virtual/vp1')
    expect(cfg.data).toEqual({ name: '新名', formula: 'a + b' })
  })

  // 锁定删除契约：DELETE /api/points/virtual/:id，且 id 做 URL 编码
  it('删除虚拟点位走 DELETE /api/points/virtual/:id，id 做 URL 编码', async () => {
    requestMock.mockResolvedValue(undefined)

    await deleteVirtualPoint('vp/1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/points/virtual/vp%2F1')
  })

  // 锁定重算契约：POST /api/points/virtual/:id/recalc，返回值压缩为 { value, error }
  it('重算虚拟点位走 POST /api/points/virtual/:id/recalc，只回传 value/error', async () => {
    requestMock.mockResolvedValue(virtualRow({ last_value: 42, error: null }))

    const res = await recalcVirtualPointView('vp1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/virtual/vp1/recalc')
    expect(res).toEqual({ value: 42, error: undefined })
    expect(res.error).toBeUndefined()
  })

  // 锁定重算失败分支：last_value 为 null 且 error 非空时只回传 error
  it('重算失败时 value 为 undefined，error 原样透传', async () => {
    requestMock.mockResolvedValue(virtualRow({ last_value: null, error: '公式错误' }))

    const res = await recalcVirtualPointView('vp1')

    expect(res.value).toBeUndefined()
    expect(res.error).toBe('公式错误')
  })
})

describe('pointsOps 批量查找替换契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定搜索契约：POST /api/points/find-replace/search，请求体 { keyword, scope }
  it('查找走 POST /api/points/find-replace/search，请求体为 { keyword, scope }', async () => {
    requestMock.mockResolvedValue([])

    await findReplaceSearchView('40001', 'address')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/find-replace/search')
    expect(cfg.data).toEqual({ keyword: '40001', scope: 'address' })
  })

  // 锁定搜索结果映射：newValue 恒为 null、selected 恒为 false（由前端本地维护）
  it('搜索结果映射为 newValue=null / selected=false', async () => {
    const raw: SearchResult[] = [
      { id: 'p1', field: 'address', device_name: '温度计', point_name: 'PT01', old_value: '40001' },
    ]
    requestMock.mockResolvedValue(raw)

    const rows = await findReplaceSearchView('40001', 'address')

    expect(rows[0]).toEqual({
      id: 'p1',
      field: 'address',
      deviceName: '温度计',
      pointName: 'PT01',
      oldValue: '40001',
      newValue: null,
      selected: false,
    })
  })

  // 锁定应用契约：POST /api/points/find-replace/apply，只提交 selected=true 的 id
  it('应用替换走 POST /api/points/find-replace/apply，只提交 selected 的 id', async () => {
    requestMock.mockResolvedValue({ updated: 2 })

    const res = await findReplaceApplyView(
      [
        { id: 'p1', field: 'address', deviceName: 'd', pointName: 'n', oldValue: '1', newValue: null, selected: true },
        { id: 'p2', field: 'address', deviceName: 'd', pointName: 'n', oldValue: '2', newValue: null, selected: false },
      ],
      'address',
      '40001',
      '40002',
    )

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/find-replace/apply')
    expect(cfg.data).toEqual({ ids: ['p1'], field: 'address', from: '40001', to: '40002' })
    // 后端返回 updated，视图层改名为 count
    expect(res).toEqual({ count: 2 })
  })
})

describe('pointsOps 批量偏移契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定列表契约：GET /api/points/offset
  it('偏移列表走 GET /api/points/offset，data_type → type', async () => {
    const raw: OffsetPointView[] = [
      { id: 'p1', name: 'PT01', old_addr: 40001, new_addr: 40002, data_type: 'u16', selected: false },
    ]
    requestMock.mockResolvedValue(raw)

    const rows = await listOffsetPointsView()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/points/offset')
    expect(rows[0]).toEqual({
      id: 'p1',
      name: 'PT01',
      oldAddr: 40001,
      newAddr: 40002,
      type: 'u16',
      selected: false,
    })
  })

  // 锁定应用契约：POST /api/points/offset/apply，请求体 { ids, offset }，只提交 selected 的 id
  it('应用偏移走 POST /api/points/offset/apply，请求体为 { ids, offset }', async () => {
    requestMock.mockResolvedValue({ updated: 1 })

    const res = await applyOffsetView(
      [
        { id: 'p1', name: 'PT01', oldAddr: 1, newAddr: 2, type: 'u16', selected: true },
        { id: 'p2', name: 'PT02', oldAddr: 1, newAddr: 2, type: 'u16', selected: false },
      ],
      10,
    )

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/offset/apply')
    expect(cfg.data).toEqual({ ids: ['p1'], offset: 10 })
    expect(res).toEqual({ updated: 1 })
  })
})

describe('pointsOps 批量克隆契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定源/目标设备列表契约：两者共用同一端点 GET /api/points/clone/devices
  it('源设备与目标设备列表共用 GET /api/points/clone/devices', async () => {
    requestMock.mockResolvedValue([])

    await listCloneSourcesView()
    expect(firstCallConfig().url).toBe('/api/points/clone/devices')

    requestMock.mockReset()
    requestMock.mockResolvedValue([])
    await listCloneTargetsView()
    expect(firstCallConfig().url).toBe('/api/points/clone/devices')
  })

  // 锁定视图映射：源视图 points 恒为空数组（后端未提供点位预览），目标视图 selected 恒为 false
  it('源视图 points 为空数组，目标视图 selected=false', async () => {
    const raw: CloneDevice[] = [{ id: 'dev-1', name: '温度计', point_count: 3 }]
    requestMock.mockResolvedValue(raw)

    const sources = await listCloneSourcesView()
    expect(sources[0]).toEqual({ id: 'dev-1', name: '温度计', pointCount: 3, points: [] })

    requestMock.mockReset()
    requestMock.mockResolvedValue(raw)
    const targets = await listCloneTargetsView()
    expect(targets[0]).toEqual({ deviceId: 'dev-1', deviceName: '温度计', selected: false, pointCount: 3 })
  })

  // 锁定克隆应用契约：POST /api/points/clone/apply，请求体字段名为 source_device_id/target_device_ids/name_suffix
  it('应用克隆走 POST /api/points/clone/apply，请求体用 snake_case 字段名', async () => {
    requestMock.mockResolvedValue({ total: 1, targets: ['dev-2'] })

    const res = await cloneApplyView(
      'dev-1',
      [
        { deviceId: 'dev-2', deviceName: '压力计', selected: true, pointCount: 3 },
        { deviceId: 'dev-3', deviceName: '流量计', selected: false, pointCount: 3 },
      ],
      '_copy',
    )

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/clone/apply')
    expect(cfg.data).toEqual({
      source_device_id: 'dev-1',
      target_device_ids: ['dev-2'],
      name_suffix: '_copy',
    })
    expect(res).toEqual({ total: 1, targets: ['dev-2'] })
  })
})
