/**
 * 「需要关注」信号派生的判定契约。
 *
 * B1 之后健康类判定全部来自后端 `/api/health/report` 的 problems / status，
 * 前端这里只负责严重度映射（critical→error，warning/info→warning）、
 * 处理入口路由与配置体检条目。阈值本身不在这里，避免前端再算一套。
 */
import { describe, it, expect } from 'vitest'
import { ref, computed } from 'vue'
import { buildHealthSignals, useHealthSignals } from './useHealthSignals'
import type { HealthProblem, HealthReport } from '@/api/status'

function report(patch: Partial<HealthReport> = {}): HealthReport {
  return {
    generated_at_ms: 1,
    status: 'ok',
    components: { process: 'ok', mqtt: 'ok', queue: 'ok', ota: 'ok', storage: 'ok' },
    summary: {
      total_devices: 10,
      collecting_devices: 10,
      online_devices: 10,
      bus_count: 2,
      queue_len: 0,
      total_reads: 100,
      success_rate: 100,
      disk_level: 'ok',
      disk_free_pct: 72.5,
      disk_thresholds: { warn: 25, critical: 15, blocking: 10 },
    },
    buses: [],
    problems: [],
    ...patch,
  }
}

function problem(level: HealthProblem['level'], code: string): HealthProblem {
  return { level, code, message: `${code} 描述`, hint: `${code} 处置提示` }
}

function build(patch: Partial<Parameters<typeof buildHealthSignals>[0]> = {}) {
  return buildHealthSignals({ report: report(), ...patch })
}

function ids(list: ReturnType<typeof build>): string[] {
  return list.map((s) => s.id)
}

describe('buildHealthSignals（基于健康聚合报告）', () => {
  it('全绿时不产出任何异常', () => {
    expect(build()).toEqual([])
  })

  it('网关健康检查未通过时产出 error 条目并指向健康巡检', () => {
    const list = build({ gatewayOnline: false })
    const s = list.find((x) => x.id === 'gateway-offline')!
    expect(s.severity).toBe('error')
    expect(s.to).toEqual({ path: '/ops', query: { tab: 'gateway-diag' } })
  })

  it('报告里的 critical problem 映射为 error，并按问题码给出处理入口', () => {
    const list = build({
      report: report({ status: 'critical', problems: [problem('critical', 'MQTT_DISCONNECTED')] }),
    })
    const s = list[0]!
    expect(s.severity).toBe('error')
    expect(s.title).toContain('MQTT_DISCONNECTED 描述')
    expect(s.detail).toContain('MQTT_DISCONNECTED 处置提示')
    expect(s.to).toEqual({ path: '/uplink', query: { tab: 'north-status' } })
  })

  it('队列 / 总线问题指向各自的处置页', () => {
    const list = build({
      report: report({ status: 'critical', problems: [problem('critical', 'QUEUE_BACKPRESSURE')] }),
    })
    expect(list[0]!.to).toEqual({ path: '/ops', query: { tab: 'task-diagnosis' } })

    const bus = build({
      report: report({ status: 'degraded', problems: [problem('warning', 'BUS_ALL_OFFLINE')] }),
    })
    expect(bus[0]!.to).toEqual({ path: '/devices' })
  })

  it('未登记的问题码退化为「查看健康报告」，不吞掉后端新增问题', () => {
    const list = build({
      report: report({ status: 'degraded', problems: [problem('warning', 'NEW_CODE')] }),
    })
    expect(list[0]!.to).toEqual({ path: '/ops', query: { tab: 'health-report' } })
  })

  it('info 级问题（如 OTA 升级中）按提醒展示，不静默丢弃', () => {
    const list = build({
      report: report({ status: 'ok', problems: [problem('info', 'OTA_IN_PROGRESS')] }),
    })
    expect(list).toHaveLength(1)
    expect(list[0]!.severity).toBe('warning')
    expect(list[0]!.to).toEqual({ path: '/system', query: { tab: 'ota' } })
  })

  it('同一问题码重复出现时 id 唯一（多条总线不互相覆盖）', () => {
    const list = build({
      report: report({
        status: 'critical',
        problems: [problem('critical', 'BUS_ALL_OFFLINE'), problem('critical', 'BUS_ALL_OFFLINE')],
      }),
    })
    expect(new Set(ids(list)).size).toBe(list.length)
  })

  it('状态非 ok 但没有可处置问题时，补一条状态条目，避免总览显示「一切正常」', () => {
    const degraded = build({ report: report({ status: 'degraded' }) })
    const s = degraded.find((x) => x.id === 'health-status')!
    expect(s.severity).toBe('warning')
    expect(s.title).toContain('降级')

    const critical = build({ report: report({ status: 'critical' }) })
    expect(critical.find((x) => x.id === 'health-status')!.severity).toBe('error')

    // ok 时不补状态条目
    expect(build({ report: report({ status: 'ok' }) }).some((x) => x.id === 'health-status')).toBe(false)
  })

  it('已有 problems 时不再叠加状态条目（问题清单本身已解释状态）', () => {
    const list = build({
      report: report({ status: 'degraded', problems: [problem('warning', 'BUS_PARTIAL_OFFLINE')] }),
    })
    expect(list.some((x) => x.id === 'health-status')).toBe(false)
  })

  it('报告尚未加载时不产出健康条目', () => {
    expect(build({ report: null })).toEqual([])
  })

  it('配置体检的 error / warning 并入异常面板，info 不占位', () => {
    const issue = (id: string, level: 'error' | 'warning' | 'info') => ({
      id, level, category: '模板不存在', target: '设备A',
      description: '模板「x」不存在', suggestion: '改成已有模板', autoFixable: false,
    })
    const list = build({ configIssues: [issue('tpl_missing_a', 'error'), issue('tip', 'info')] })
    const s = list.find((x) => x.id === 'cfg-tpl_missing_a')!
    expect(s.severity).toBe('error')
    expect(s.title).toContain('模板不存在')
    expect(s.detail).toContain('改成已有模板')
    // info 属于改进建议，不该出现在「需要关注」里
    expect(list.some((x) => x.id.startsWith('cfg-') && x.id !== 'cfg-tpl_missing_a')).toBe(false)
    expect(s.to).toEqual({ path: '/system/config', query: { tab: 'config-health' } })
  })

  it('配置问题超过 3 条时只列前 3 条并给汇总入口', () => {
    const many = Array.from({ length: 5 }, (_, i) => ({
      id: `i${i}`, level: 'warning' as const, category: '重复地址', target: `d${i}`,
      description: '地址重复', suggestion: '改地址', autoFixable: false,
    }))
    const list = build({ configIssues: many })
    const cfgIds = ids(list).filter((id) => id.startsWith('cfg-'))
    expect(cfgIds).toEqual(['cfg-i0', 'cfg-i1', 'cfg-i2', 'cfg-more'])
    expect(list.find((x) => x.id === 'cfg-more')!.title).toContain('另有 2 项')
  })

  it('error 排在 warning 之前，同级保持后端 problems 顺序', () => {
    const list = build({
      report: report({
        status: 'critical',
        problems: [problem('warning', 'BUS_PARTIAL_OFFLINE'), problem('critical', 'MQTT_DISCONNECTED')],
      }),
    })
    expect(list.map((s) => s.severity)).toEqual(['error', 'warning'])
  })
})

describe('useHealthSignals', () => {
  it('源数据变化后派生结果随之更新', async () => {
    const r = ref<HealthReport | null>(report())
    const online = ref(true)
    const signals = useHealthSignals(r, online)

    expect(signals.value).toEqual([])

    r.value = report({ status: 'degraded', problems: [problem('warning', 'BUS_PARTIAL_OFFLINE')] })
    await Promise.resolve()
    expect(signals.value).toHaveLength(1)

    r.value = null
    online.value = false
    await Promise.resolve()
    expect(signals.value.map((s) => s.id)).toEqual(['gateway-offline'])
  })

  it('也接受 getter 作为数据源', () => {
    const r = ref<HealthReport | null>(
      report({ status: 'critical', problems: [problem('critical', 'QUEUE_BACKPRESSURE')] }),
    )
    const signals = useHealthSignals(() => r.value, () => true, () => [])
    expect(signals.value.find((s) => s.id.startsWith('problem-QUEUE'))).toBeTruthy()
  })

  it('接受 computed 作为数据源', () => {
    const r = ref<HealthReport | null>(report({ status: 'degraded' }))
    const signals = useHealthSignals(computed(() => r.value), computed(() => true))
    expect(signals.value.find((s) => s.id === 'health-status')).toBeTruthy()
  })
})
