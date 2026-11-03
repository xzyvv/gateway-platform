/**
 * 点位候选来源契约：配置点位 ∪ 运行时快照。
 *
 * 背景：这些页面的点位下拉此前只读 point-quality（运行时快照），
 * 新设备接入、采集器还没跑起来时该接口返回空数组 → 下拉整片空白，
 * 用户连「先选点位」都做不到。这里锁死合并规则：
 *   - 配置里有、运行时没有 → 仍可选中（值/质量为 null）
 *   - 运行时里有、配置里没有（模板展开点位）→ 也要出现
 *   - 两处都有 → 只出现一次，且用运行时的当前值/质量
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { usePointOptions } from './usePointOptions'

const m = vi.hoisted(() => ({
  getConfig: vi.fn(),
  listPointQuality: vi.fn(),
}))

vi.mock('@/api/config', () => ({ getConfig: () => m.getConfig() }))
vi.mock('@/api/dataQuality', () => ({ listPointQuality: () => m.listPointQuality() }))

describe('usePointOptions', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('没有运行时快照时仍能把配置点位作为候选', async () => {
    m.getConfig.mockResolvedValue({
      devices: [{ id: 'd1', name: '电表1' }],
      points: [{ id: 'p1', name: '电压', device_id: 'd1' }],
    })
    m.listPointQuality.mockResolvedValue([])

    const { pointOptions, reload } = usePointOptions()
    await reload()

    expect(pointOptions.value).toHaveLength(1)
    expect(pointOptions.value[0]).toMatchObject({
      id: 'p1', name: '电压', deviceName: '电表1', value: null, quality: null,
    })
  })

  it('运行时快照的点位（模板展开）也在候选里，且不与配置点位重复', async () => {
    m.getConfig.mockResolvedValue({
      devices: [{ id: 'd1', name: '电表1' }],
      points: [{ id: 'p1', name: '电压', device_id: 'd1' }],
    })
    m.listPointQuality.mockResolvedValue([
      { point_id: 'p1', point_name: '电压', value: 220, quality: 'good', device_id: 'd1' },
      { point_id: 'p2', point_name: '电流', value: 5, quality: 'good', device_id: 'd1' },
    ])

    const { pointOptions, reload } = usePointOptions()
    await reload()

    expect(pointOptions.value.map((p) => p.id).sort()).toEqual(['p1', 'p2'])
    // 两处都有的点位以运行时为准（带当前值）
    expect(pointOptions.value.find((p) => p.id === 'p1')!.value).toBe(220)
  })

  it('两路请求都失败时返回空列表而不抛错', async () => {
    m.getConfig.mockRejectedValue(new Error('boom'))
    m.listPointQuality.mockRejectedValue(new Error('boom'))

    const { pointOptions, reload } = usePointOptions()
    await reload()

    expect(pointOptions.value).toEqual([])
  })
})
