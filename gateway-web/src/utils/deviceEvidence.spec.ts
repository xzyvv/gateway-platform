import { describe, it, expect } from 'vitest'
import {
  aggregateBackoffByDevice,
  aggregateLastFrameByDevice,
  mergeDeviceEvidence,
} from './deviceEvidence'
import type { PointBackoff } from '@/api/dataQuality'
import type { Packet } from '@/api/protocolTools/types'

/**
 * 回归测试：设备维度证据聚合（P2 #5）。
 *
 * 目标：让「某设备为什么离线」在一屏可答——需要三类证据同时到位：
 *   1. 最后成功采集时间（来自退避统计）
 *   2. 连续失败次数（来自退避统计）
 *   3. 最近原始帧 hex + 失败原因（来自抓包缓冲）
 */

function mkBackoff(over: Partial<PointBackoff> = {}): PointBackoff {
  return {
    point_id: 'p1',
    point_name: 'P1',
    device_id: 'dev-a',
    device_name: '设备A',
    bus: 'bus1',
    base_interval_ms: 1000,
    current_interval_ms: 1000,
    backoff_multiplier: 1,
    consecutive_failures: 0,
    stale_count: 0,
    last_good_value: 1,
    last_good_ts: 0,
    quality: 'good',
    backing_off: false,
    ...over,
  }
}

function mkPacket(over: Partial<Packet> = {}): Packet {
  return {
    id: 'pkt_1',
    time: 1000,
    direction: 'response',
    deviceId: 'dev-a',
    deviceName: '设备A',
    bus: 'bus1',
    hex: '01 03 02 00 2A',
    ascii: '.',
    parsed: '寄存器值: 42',
    ...over,
  }
}

describe('退避统计按设备聚合', () => {
  it('取该设备下各点位最后成功时间的最大值', () => {
    const map = aggregateBackoffByDevice([
      mkBackoff({ point_id: 'p1', last_good_ts: 1000 }),
      mkBackoff({ point_id: 'p2', last_good_ts: 5000 }),
      mkBackoff({ point_id: 'p3', last_good_ts: 3000 }),
    ])
    expect(map['dev-a']?.lastGoodTs).toBe(5000)
  })

  it('取该设备下各点位连续失败数的最大值', () => {
    const map = aggregateBackoffByDevice([
      mkBackoff({ point_id: 'p1', consecutive_failures: 2 }),
      mkBackoff({ point_id: 'p2', consecutive_failures: 7 }),
    ])
    expect(map['dev-a']?.consecutiveFailures).toBe(7)
  })

  it('无 device_id 的点位被跳过（不猜归属）', () => {
    const map = aggregateBackoffByDevice([mkBackoff({ device_id: '' })])
    expect(Object.keys(map)).toHaveLength(0)
  })

  it('从未成功过的设备 lastGoodTs 为 0（区别于「有数据但很久没成功」）', () => {
    const map = aggregateBackoffByDevice([mkBackoff({ last_good_ts: 0 })])
    expect(map['dev-a']?.lastGoodTs).toBe(0)
  })
})

describe('抓包缓冲按设备聚合最近帧', () => {
  it('取时间最新的一帧', () => {
    const map = aggregateLastFrameByDevice([
      mkPacket({ id: 'a', time: 1000, hex: 'AA' }),
      mkPacket({ id: 'b', time: 3000, hex: 'BB' }),
      mkPacket({ id: 'c', time: 2000, hex: 'CC' }),
    ])
    expect(map['dev-a']?.lastFrameHex).toBe('BB')
    expect(map['dev-a']?.lastFrameTs).toBe(3000)
  })

  it('同时间戳时请求帧优先（排障时先看发了什么）', () => {
    const map = aggregateLastFrameByDevice([
      mkPacket({ id: 'r', time: 1000, direction: 'request', hex: 'REQ' }),
      mkPacket({ id: 'p', time: 1000, direction: 'response', hex: 'RESP' }),
    ])
    expect(map['dev-a']?.lastFrameHex).toBe('REQ')
    expect(map['dev-a']?.lastFrameDirection).toBe('request')
  })

  it('无 device_id 的帧被跳过（不把别的设备的帧挂到本设备）', () => {
    const map = aggregateLastFrameByDevice([mkPacket({ deviceId: '' })])
    expect(Object.keys(map)).toHaveLength(0)
  })

  it('识别带错误标记的帧为失败并提取原因', () => {
    const map = aggregateLastFrameByDevice([
      mkPacket({ parsed: '错误: CRC 校验失败' }),
    ])
    expect(map['dev-a']?.lastFrameFailed).toBe(true)
    expect(map['dev-a']?.lastFrameError).toBe('CRC 校验失败')
  })

  it('正常帧不算失败', () => {
    const map = aggregateLastFrameByDevice([mkPacket({ parsed: '寄存器值: 42' })])
    expect(map['dev-a']?.lastFrameFailed).toBe(false)
    expect(map['dev-a']?.lastFrameError).toBe('')
  })

  it('异常码格式（异常码: 0x02）也能提取', () => {
    const map = aggregateLastFrameByDevice([mkPacket({ parsed: '异常码: 0x02 非法数据地址' })])
    expect(map['dev-a']?.lastFrameError).toContain('0x02')
  })
})

describe('合并两类证据源', () => {
  it('退避统计与抓包帧合并到同一设备条目', () => {
    const merged = mergeDeviceEvidence(
      aggregateBackoffByDevice([mkBackoff({ last_good_ts: 9000, consecutive_failures: 3 })]),
      aggregateLastFrameByDevice([mkPacket({ hex: 'DE AD BE EF' })]),
    )
    const e = merged['dev-a']
    expect(e?.lastGoodTs).toBe(9000)
    expect(e?.consecutiveFailures).toBe(3)
    expect(e?.lastFrameHex).toBe('DE AD BE EF')
  })

  it('只有抓包帧时，退避统计字段用默认值兜底', () => {
    const merged = mergeDeviceEvidence({}, aggregateLastFrameByDevice([mkPacket()]))
    expect(merged['dev-a']?.lastGoodTs).toBe(0)
    expect(merged['dev-a']?.consecutiveFailures).toBe(0)
    expect(merged['dev-a']?.lastFrameHex).toBe('01 03 02 00 2A')
  })

  it('两边都没有的设备不出现在结果里', () => {
    const merged = mergeDeviceEvidence({}, {})
    expect(Object.keys(merged)).toHaveLength(0)
  })
})
