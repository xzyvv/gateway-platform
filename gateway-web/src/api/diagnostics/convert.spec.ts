/**
 * 采集统计报表 raw → View 转换测试
 *
 * 重点锁定「每总线四指标」（在线设备数 / 超时率 / 最近错误码 / 重连次数）的
 * 字段映射：漏映射一个字段，前端表格该列就会整列空白（undefined），
 * 而这种问题在类型层面不会被 tsc 抓到（字段可选 / snake_case 拼错只在运行时暴露）。
 */
import { describe, it, expect } from 'vitest'
import { toStatsReportView } from './convert'
import type { StatsReport } from './raw'

function busStat(over: Partial<StatsReport['bus_stats'][number]> = {}) {
  return {
    bus: 'COM1',
    protocol: 'modbus',
    total_reads: 10,
    success: 9,
    failed: 1,
    success_rate: 90,
    avg_latency_ms: 12,
    device_count: 2,
    online_devices: 2,
    timeout_rate: 10,
    last_error_code: 'TIMEOUT',
    reconnect_count: 3,
    ...over,
  }
}

function report(buses: StatsReport['bus_stats']): StatsReport {
  return {
    period: 'day',
    start_at_ms: 1_700_000_000_000,
    end_at_ms: 1_700_086_400_000,
    device_stats: [],
    bus_stats: buses,
    error_dist: [],
    summary: {
      total_reads: 10,
      success_rate: 90,
      avg_latency_ms: 12,
      total_data_points: 9,
    },
  }
}

describe('toStatsReportView（每总线四指标映射）', () => {
  it('四个关键字段逐一映射到 View 字段名', () => {
    const view = toStatsReportView(report([busStat()]))
    const bus = view.busStats[0]!

    expect(bus.onlineDevices).toBe(2)
    expect(bus.timeoutRate).toBe(10)
    expect(bus.lastErrorCode).toBe('TIMEOUT')
    expect(bus.reconnectCount).toBe(3)
  })

  it('无采集时保留 null（前端渲染「—」），不得退化成 0', () => {
    const view = toStatsReportView(
      report([
        busStat({
          online_devices: 0,
          timeout_rate: null,
          last_error_code: null,
          reconnect_count: 0,
        }),
      ]),
    )
    const bus = view.busStats[0]!

    expect(bus.onlineDevices).toBe(0)
    expect(bus.timeoutRate).toBeNull()
    expect(bus.lastErrorCode).toBeNull()
    expect(bus.reconnectCount).toBe(0)
  })

  it('Modbus 异常码原样透传（EXC_02 形式）', () => {
    const view = toStatsReportView(report([busStat({ last_error_code: 'EXC_02' })]))
    expect(view.busStats[0]!.lastErrorCode).toBe('EXC_02')
  })

  it('多条总线各自独立映射', () => {
    const view = toStatsReportView(
      report([
        busStat({ bus: 'COM1', reconnect_count: 1, last_error_code: 'CRC' }),
        busStat({ bus: 'COM2', reconnect_count: 7, last_error_code: 'IO', online_devices: 0 }),
      ]),
    )
    expect(view.busStats).toHaveLength(2)
    expect(view.busStats[0]).toMatchObject({
      bus: 'COM1',
      reconnectCount: 1,
      lastErrorCode: 'CRC',
    })
    expect(view.busStats[1]).toMatchObject({
      bus: 'COM2',
      reconnectCount: 7,
      lastErrorCode: 'IO',
      onlineDevices: 0,
    })
  })

  it('时间戳转 number（ts-rs bigint → number）', () => {
    const view = toStatsReportView(report([busStat()]))
    expect(view.startAt).toBe(1_700_000_000_000)
    expect(view.endAt).toBe(1_700_086_400_000)
  })
})
