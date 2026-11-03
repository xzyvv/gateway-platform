/**
 * 健康聚合报告共享快照的契约。
 *
 * 四个页面共用这一份快照，判定的是「会不会重复打接口」与「断网时是否清空数据」，
 * 因此逐条锁死：
 *   - TTL 内复用、超时才重新请求
 *   - 并发调用共享同一个 inflight Promise（只发一次）
 *   - 失败保留上一次成功结果，同时置 error
 *   - invalidate 后绕过 TTL 重新请求
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import type { HealthProblem, HealthReport } from '@/api/status'

const m = vi.hoisted(() => ({ getHealthReport: vi.fn() }))

vi.mock('@/api/status', () => ({
  getHealthReport: () => m.getHealthReport(),
}))

function makeReport(
  status: HealthReport['status'],
  problems: HealthProblem[] = [],
): HealthReport {
  return {
    generated_at_ms: 1,
    status,
    components: { process: 'ok', mqtt: status === 'critical' ? 'down' : 'ok' },
    summary: {
      total_devices: 2,
      collecting_devices: 2,
      online_devices: 1,
      bus_count: 1,
      queue_len: 0,
      total_reads: 10,
      success_rate: 90,
    },
    buses: [],
    problems,
  }
}

describe('useHealthReport', () => {
  let mod: typeof import('./useHealthReport')
  let now: number

  beforeEach(async () => {
    vi.resetModules()
    m.getHealthReport.mockReset()
    // TTL 基于 Date.now；用可控时钟推进，避免依赖真实等待
    now = 1_000
    vi.spyOn(Date, 'now').mockImplementation(() => now)
    // D3：快照已迁入 Pinia；resetModules 后必须重新取 pinia 实例，
    // 否则本测试的 createPinia 与被测模块内的 pinia 是两份互不相知的模块实例。
    const { createPinia, setActivePinia } = await import('pinia')
    setActivePinia(createPinia())
    mod = await import('./useHealthReport')
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it('TTL 内复用快照，超过 10s 才重新请求', async () => {
    const first = makeReport('ok')
    m.getHealthReport.mockResolvedValue(first)
    const { report, refresh } = mod.useHealthReport()

    await refresh()
    expect(m.getHealthReport).toHaveBeenCalledTimes(1)
    expect(report.value).toEqual(first)

    // TTL 内（含第 9.9s）复用，不再请求
    now += 9_900
    await refresh()
    expect(m.getHealthReport).toHaveBeenCalledTimes(1)

    // 超过 10s 后重新请求
    now += 200
    const second = makeReport('degraded')
    m.getHealthReport.mockResolvedValue(second)
    await refresh()
    expect(m.getHealthReport).toHaveBeenCalledTimes(2)
    expect(report.value).toEqual(second)
  })

  it('并发调用共享同一个请求，只发一次', async () => {
    let resolveRequest!: (v: HealthReport) => void
    m.getHealthReport.mockReturnValue(new Promise<HealthReport>((r) => { resolveRequest = r }))
    const { report, refresh, loading } = mod.useHealthReport()

    const p1 = refresh()
    const p2 = refresh()
    const p3 = refresh()
    expect(m.getHealthReport).toHaveBeenCalledTimes(1)
    expect(loading.value).toBe(true)

    const snapshot = makeReport('ok')
    resolveRequest(snapshot)
    await Promise.all([p1, p2, p3])

    expect(report.value).toEqual(snapshot)
    expect(loading.value).toBe(false)
    expect(m.getHealthReport).toHaveBeenCalledTimes(1)
  })

  it('失败保留上一次成功结果并置 error，不是把页面清空', async () => {
    const ok = makeReport('ok')
    m.getHealthReport.mockResolvedValue(ok)
    const { report, error, refresh } = mod.useHealthReport()
    await refresh()
    expect(report.value).toEqual(ok)

    m.getHealthReport.mockRejectedValue(new Error('网络不可达'))
    mod.invalidateHealthReport()
    await refresh()

    expect(report.value).toEqual(ok) // 旧快照仍在
    expect(error.value).toBe('网络不可达')

    // 失败后 TTL 仍生效：10s 内再次调用不会再打接口
    await refresh()
    expect(m.getHealthReport).toHaveBeenCalledTimes(2)
  })

  it('失败后成功会清除 error', async () => {
    const { error, refresh } = mod.useHealthReport()
    m.getHealthReport.mockRejectedValue(new Error('boom'))
    await refresh()
    expect(error.value).toBe('boom')

    m.getHealthReport.mockResolvedValue(makeReport('ok'))
    mod.invalidateHealthReport()
    await refresh()
    expect(error.value).toBeNull()
  })

  it('invalidate 后重新请求（写操作后的标准用法）', async () => {
    m.getHealthReport.mockResolvedValue(makeReport('ok'))
    const { refresh } = mod.useHealthReport()

    await refresh()
    await refresh() // TTL 内复用
    expect(m.getHealthReport).toHaveBeenCalledTimes(1)

    mod.invalidateHealthReport()
    await refresh()
    expect(m.getHealthReport).toHaveBeenCalledTimes(2)
  })

  it('refresh(force) 绕过 TTL，手动刷新总能拿到新数据', async () => {
    m.getHealthReport.mockResolvedValue(makeReport('ok'))
    const { refresh } = mod.useHealthReport()

    await refresh()
    expect(m.getHealthReport).toHaveBeenCalledTimes(1)

    m.getHealthReport.mockResolvedValue(makeReport('critical'))
    await refresh(true)
    expect(m.getHealthReport).toHaveBeenCalledTimes(2)
  })

  it('problemCounts 按 critical/warning/info 分级计数', async () => {
    m.getHealthReport.mockResolvedValue(
      makeReport('critical', [
        { level: 'critical', code: 'A', message: 'm', hint: 'h' },
        { level: 'warning', code: 'B', message: 'm', hint: 'h' },
        { level: 'warning', code: 'C', message: 'm', hint: 'h' },
        { level: 'info', code: 'D', message: 'm', hint: 'h' },
      ]),
    )
    const { refresh, problemCounts } = mod.useHealthReport()
    await refresh()
    expect(problemCounts.value).toEqual({ critical: 1, warning: 2, info: 1 })
  })

  it('overallStatus 给出后端原词 + 统一中文标签与 el-tag type', async () => {
    const { refresh, overallStatus } = mod.useHealthReport()

    // 尚未加载
    expect(overallStatus.value).toEqual({ value: 'unknown', label: '暂无数据', type: 'info' })

    m.getHealthReport.mockResolvedValue(makeReport('degraded'))
    await refresh()
    expect(overallStatus.value).toEqual({ value: 'degraded', label: '降级', type: 'warning' })

    m.getHealthReport.mockResolvedValue(makeReport('critical'))
    await refresh(true)
    expect(overallStatus.value).toEqual({ value: 'critical', label: '严重', type: 'danger' })

    m.getHealthReport.mockResolvedValue(makeReport('ok'))
    await refresh(true)
    expect(overallStatus.value).toEqual({ value: 'ok', label: '正常', type: 'success' })
  })

  it('多个调用方看到同一份快照（Pinia Store 共享）', async () => {
    const snapshot = makeReport('ok')
    m.getHealthReport.mockResolvedValue(snapshot)
    const a = mod.useHealthReport()
    const b = mod.useHealthReport()

    await a.refresh()
    await b.refresh()

    expect(m.getHealthReport).toHaveBeenCalledTimes(1)
    expect(a.report.value).toBe(b.report.value)
    expect(b.report.value).toEqual(snapshot)
  })
})
