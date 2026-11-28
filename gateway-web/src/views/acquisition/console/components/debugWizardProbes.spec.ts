import { describe, it, expect } from 'vitest'
import {
  isRelevantProblem,
  levelTagType,
  runBusProbe,
  runDeviceProbe,
  runHealthProbe,
} from './debugWizardProbes'
import type { BusConfig, DeviceConfig } from '@/api/types'
import type { HealthProblem } from '@/api/status'

/**
 * 自动检查（探针）逻辑的判定契约。
 *
 * 这些判定原先内联在 DebugWizard.vue 里，只被页面级测试间接覆盖；
 * 拆成纯函数模块后可以逐条钉住「什么输入 → 什么结论 + 什么下一步入口」。
 */

function bus(overrides: Partial<BusConfig> = {}): BusConfig {
  return { port_name: 'bus-1', protocol: 'modbus', ...overrides } as BusConfig
}

function device(overrides: Partial<DeviceConfig> = {}): DeviceConfig {
  return { id: 'dev-1', name: '电表一号', ...overrides } as DeviceConfig
}

function problem(overrides: Partial<HealthProblem> = {}): HealthProblem {
  return {
    level: 'warning',
    code: 'TIMEOUT_HIGH',
    message: '采集超时率偏高',
    hint: '',
    ...overrides,
  } as HealthProblem
}

describe('debugWizardProbes — 总线探针', () => {
  it('没有任何总线时给出「去配置管理」入口', () => {
    const r = runBusProbe({ buses: [], selectedBus: '', protocol: 'modbus-rtu' })
    expect(r.tone).toBe('warning')
    expect(r.message).toContain('未配置总线')
    expect(r.goto?.to).toBe('/system/config?tab=wizard')
  })

  it('有总线但未选中时提示先选总线', () => {
    const r = runBusProbe({ buses: [bus()], selectedBus: '', protocol: 'modbus-rtu' })
    expect(r.tone).toBe('warning')
    expect(r.message).toContain('尚未选中调试总线')
    expect(r.goto?.to).toBe('/acquisition/console?tab=packet-editor')
  })

  it('协议不匹配时点名两边的协议并给出去切换入口', () => {
    const r = runBusProbe({
      buses: [bus({ protocol: 'modbus', tcp_addr: '192.168.1.10:502' })],
      selectedBus: 'bus-1',
      protocol: 'modbus-rtu',
    })
    expect(r.tone).toBe('warning')
    expect(r.message).toContain('bus-1')
    expect(r.message).toContain('Modbus TCP')
    expect(r.message).toContain('Modbus RTU')
    expect(r.message).toContain('不匹配')
  })

  it('协议匹配时给出成功结论', () => {
    const r = runBusProbe({
      buses: [bus()],
      selectedBus: 'bus-1',
      protocol: 'modbus-rtu',
    })
    expect(r.tone).toBe('success')
    expect(r.message).toContain('与本次排查协议匹配')
  })
})

describe('debugWizardProbes — 设备探针', () => {
  it('没有任何设备时给出「去设备管理」入口', () => {
    const r = runDeviceProbe({ devices: [], selectedDeviceId: '' })
    expect(r.tone).toBe('warning')
    expect(r.message).toContain('尚未配置任何设备')
    expect(r.goto?.to).toBe('/devices?tab=devices')
  })

  it('有设备但未选中时提示先选设备，并说明已加载几台', () => {
    const r = runDeviceProbe({ devices: [device(), device({ id: 'dev-2', name: '压力表' })], selectedDeviceId: '' })
    expect(r.tone).toBe('warning')
    expect(r.message).toContain('2 台设备')
    expect(r.message).toContain('未选中')
  })

  it('选中设备后给出设备名与 ID', () => {
    const r = runDeviceProbe({ devices: [device()], selectedDeviceId: 'dev-1' })
    expect(r.tone).toBe('success')
    expect(r.message).toContain('电表一号')
    expect(r.message).toContain('dev-1')
  })
})

describe('debugWizardProbes — 健康探针', () => {
  it('读取失败时报 danger，而不是「未发现异常」', () => {
    const r = runHealthProbe(null, null, 'modbus-rtu', 'health 接口 500')
    expect(r.tone).toBe('danger')
    expect(r.message).toContain('健康报告读取失败')
  })

  it('无相关异常时给出成功结论', () => {
    const r = runHealthProbe({ problems: [] }, null, 'modbus-rtu', null)
    expect(r.tone).toBe('success')
    expect(r.message).toContain('未发现相关异常')
  })

  it('异常按条列出并保留等级信息', () => {
    const r = runHealthProbe(
      { problems: [problem({ level: 'critical', code: 'BUS_ALL_OFFLINE', message: '总线全部离线' })] },
      null,
      'modbus-rtu',
      null,
    )
    expect(r.tone).toBe('warning')
    expect(r.message).toContain('发现 1 条')
    expect(r.problems?.[0]?.code).toBe('BUS_ALL_OFFLINE')
  })

  it('刷新结果为空时回退到当前报告', () => {
    const r = runHealthProbe(null, { problems: [problem()] }, 'modbus-rtu', null)
    expect(r.tone).toBe('warning')
    expect(r.problems).toHaveLength(1)
  })
})

describe('debugWizardProbes — 问题相关性过滤', () => {
  it('MQTT 流程过滤掉总线类异常', () => {
    expect(isRelevantProblem(problem({ code: 'BUS_ALL_OFFLINE', message: '总线离线' }), 'mqtt')).toBe(false)
    expect(isRelevantProblem(problem({ code: 'MQTT_DISCONNECTED', message: 'MQTT 已断开' }), 'mqtt')).toBe(true)
  })

  it('串口流程过滤掉上行类异常', () => {
    expect(isRelevantProblem(problem({ code: 'QUEUE_BACKPRESSURE', message: '队列积压' }), 'modbus-rtu')).toBe(false)
    expect(isRelevantProblem(problem({ code: 'CRC_ERROR', message: 'CRC 校验失败' }), 'modbus-rtu')).toBe(true)
  })

  it('generic 流程不过滤（任何问题都相关）', () => {
    expect(isRelevantProblem(problem({ code: 'ANYTHING', message: '任意' }), 'generic')).toBe(true)
  })
})

describe('debugWizardProbes — 等级配色', () => {
  it('critical → danger，warning → warning，其余 → info', () => {
    expect(levelTagType('critical')).toBe('danger')
    expect(levelTagType('warning')).toBe('warning')
    expect(levelTagType('info')).toBe('info')
  })
})
