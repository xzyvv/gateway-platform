import { describe, expect, it } from 'vitest'
import type { GatewayConfig } from '@/api/types'
import { type ConfigFragment, mergeFragment, parseFragment, pointsIgnoredByTemplateMode } from './configFragment'

/** 测试只关心主键与变化统计，用最小对象构造片段（真实字段由后端校验兜底） */
function frag(sections: Record<string, unknown[]>): ConfigFragment {
  return sections as unknown as ConfigFragment
}

function config(partial: Record<string, unknown> = {}): GatewayConfig {
  return {
    buses: [],
    devices: null,
    points: [],
    device_groups: [],
    virtual_points: [],
    convert_rules: [],
    mqtt: { broker: 'tcp://127.0.0.1:1883' },
    ...partial,
  } as unknown as GatewayConfig
}

describe('parseFragment', () => {
  it('空内容与非法 JSON 都直接给出可读报错', () => {
    expect(parseFragment('   ')).toEqual({ ok: false, error: '请粘贴或上传要导入的配置片段' })
    const bad = parseFragment('{ points: ')
    expect(bad.ok).toBe(false)
    if (!bad.ok) expect(bad.error).toContain('JSON 解析失败')
  })

  it('拒绝顶层数组与非对象', () => {
    expect(parseFragment('[]')).toEqual({ ok: false, error: '片段必须是 JSON 对象，例如 {"points": [ ... ]}' })
    expect(parseFragment('null').ok).toBe(false)
  })

  it('拒绝标量节，并指引到整份配置导入', () => {
    const res = parseFragment('{"mqtt": {"broker": "tcp://x"}}')
    expect(res.ok).toBe(false)
    if (!res.ok) {
      expect(res.error).toContain('mqtt')
      expect(res.error).toContain('整份配置导入')
    }
  })

  it('每节必须是数组，每条必须是对象，且必须带合并主键', () => {
    const notArray = parseFragment('{"points": {}}')
    expect(notArray.ok).toBe(false)
    if (!notArray.ok) expect(notArray.error).toContain('必须是数组')

    const notObject = parseFragment('{"points": [1]}')
    expect(notObject.ok).toBe(false)
    if (!notObject.ok) expect(notObject.error).toContain('不是对象')

    const noKey = parseFragment('{"points": [{"name": "温度"}]}')
    expect(noKey.ok).toBe(false)
    if (!noKey.ok) expect(noKey.error).toContain('合并主键 id')

    const busNoKey = parseFragment('{"buses": [{"id": "com1"}]}')
    expect(busNoKey.ok).toBe(false)
    if (!busNoKey.ok) expect(busNoKey.error).toContain('port_name')
  })

  it('6 类列表节全部接受', () => {
    const res = parseFragment(
      JSON.stringify({
        buses: [{ port_name: 'com1' }],
        devices: [{ id: 'd1' }],
        points: [{ id: 'p1' }],
        device_groups: [{ id: 'g1' }],
        virtual_points: [{ id: 'v1' }],
        convert_rules: [{ id: 'r1' }],
      }),
    )
    expect(res.ok).toBe(true)
  })
})

describe('mergeFragment', () => {
  it('按 id 区分新增 / 覆盖 / 未变，并保留原顺序', () => {
    const current = config({ points: [{ id: 'p1', name: '旧温度' }, { id: 'p2', name: '压力' }] })
    const { merged, stats } = mergeFragment(
      current,
      frag({
        points: [
          { id: 'p1', name: '新温度' },
          { id: 'p2', name: '压力' },
          { id: 'p3', name: '流量' },
        ],
      }),
    )

    expect(merged.points.map((p) => p.id)).toEqual(['p1', 'p2', 'p3'])
    expect(merged.points[0]!.name).toBe('新温度')
    expect(stats).toEqual([{ section: 'points', added: 1, updated: 1, unchanged: 1 }])
  })

  it('总线按 port_name、设备按 id（缺 id 回退 name）合并', () => {
    const current = config({
      buses: [{ port_name: 'com1', baud_rate: 9600 }],
      devices: [{ name: '电表1', slave_id: 1 }],
    })
    const { merged, stats } = mergeFragment(
      current,
      frag({
        buses: [{ port_name: 'com1', baud_rate: 19200 }],
        devices: [
          { name: '电表1', slave_id: 1 },
          { name: '电表2', slave_id: 2 },
        ],
      }),
    )

    expect((merged.buses[0] as unknown as { baud_rate: number }).baud_rate).toBe(19200)
    expect(merged.devices!.map((d) => d.name)).toEqual(['电表1', '电表2'])
    expect(stats).toEqual([
      { section: 'buses', added: 0, updated: 1, unchanged: 0 },
      { section: 'devices', added: 1, updated: 0, unchanged: 1 },
    ])
  })

  it('未出现在片段里的配置节原样保留，不被清空', () => {
    const current = config({ mqtt: { broker: 'tcp://keep:1883' } })
    const { merged } = mergeFragment(current, frag({ points: [{ id: 'p1' }] }))
    expect(merged.mqtt).toEqual(current.mqtt)
    expect(merged.devices).toBeNull()
  })

  it('bigint 字段相同也算未变（不会误判为覆盖）', () => {
    const current = config({ device_groups: [{ id: 'g1', name: '一组', created_at: 1700000000000n }] })
    const { stats } = mergeFragment(
      current,
      frag({ device_groups: [{ id: 'g1', name: '一组', created_at: 1700000000000n }] }),
    )
    expect(stats).toEqual([{ section: 'device_groups', added: 0, updated: 0, unchanged: 1 }])
  })

  it('不修改传入的当前配置', () => {
    const current = config({ points: [{ id: 'p1', name: '旧' }] })
    mergeFragment(current, frag({ points: [{ id: 'p1', name: '新' }] }))
    expect(current.points[0]!.name).toBe('旧')
  })
})

describe('pointsIgnoredByTemplateMode', () => {
  it('已配置设备模板时，纯点位片段会被拦下', () => {
    const current = config({ devices: [{ id: 'd1' }] })
    expect(pointsIgnoredByTemplateMode(current, frag({ points: [{ id: 'p1' }] }))).toBe(true)
  })

  it('片段带设备或当前无设备时不拦', () => {
    const withDevices = config({ devices: [{ id: 'd1' }] })
    expect(pointsIgnoredByTemplateMode(withDevices, frag({ devices: [{ id: 'd2' }] }))).toBe(false)
    expect(pointsIgnoredByTemplateMode(config(), frag({ points: [{ id: 'p1' }] }))).toBe(false)
  })
})
