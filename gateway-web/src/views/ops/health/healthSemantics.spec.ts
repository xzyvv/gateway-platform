/**
 * 健康语义同源契约（DD-68 / DD-69 / DD-70 / DD-71 / DD-72）
 *
 * 这五项原本是同一批语义的五份独立实现：组件状态取色（健康报告页自建）、
 * 综合状态中文名（store 与总览信号各一份字面量）、磁盘水位档位字典（两页各一份）、
 * 问题分级取色（两处各一份）、问题分级计数（store 与结论条各一份 reduce）。
 * 任何一处改动都会与其余四处漂移——最典型的后果是「同一台网关在总览页显红、
 * 在健康报告页显黄」。
 *
 * 现在它们统一收敛到 `utils/healthStatus`，本文件钉住收敛后的行为：
 *   1. 组件状态：健康报告页与运行总览页逐项一致（中文名 + 徽章色）；
 *   2. `error` 档位显红（danger），不琥珀；
 *   3. 综合状态中文名：store 与总览信号同源；
 *   4. 磁盘水位档位：健康报告页与磁盘图表页同源；
 *   5. 问题分级：中文名 + 徽章色 + 计数三处同源。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

import {
  countProblemLevels,
  diskLevelMeta,
  overallStatusMeta,
  problemLevelMeta,
} from '@/utils/healthStatus'
import { healthStatusInfo } from '@/utils/healthStatus'
import HealthConclusionBar from '@/views/ops/health/HealthConclusionBar.vue'

/* -------------------------------------------------------------------------- */
/* 1) 组件状态：健康报告页 vs 运行总览页                                        */
/* -------------------------------------------------------------------------- */

const { state } = vi.hoisted(() => ({
  state: {
    report: null as Record<string, unknown> | null,
    loading: false,
    overall: { label: '正常', type: 'success', value: 'ok' },
    refresh: vi.fn(),
  },
}))

vi.mock('@/composables/system/useHealthReport', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  return {
    useHealthReport: () => ({
      report: ref(state.report),
      loading: ref(state.loading),
      overallStatus: ref(state.overall),
      refresh: state.refresh,
    }),
  }
})

import HealthReport from '@/views/ops/health/HealthReport.vue'
import HealthStatus from '@/views/overview/HealthStatus.vue'

async function settle() {
  await flushPromises()
  await nextTick()
}

/** 收集组件里所有 el-tag 的 [文本, 徽章色] */
function collectTags(wrapper: ReturnType<typeof mount>): Array<[string, string]> {
  return wrapper.findAll('.el-tag').map((el) => {
    const cls = el.classes().find((c) => c.startsWith('el-tag--'))
    return [el.text(), (cls ?? '').replace('el-tag--', '')]
  })
}

describe('组件状态取色同源（DD-68）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    state.report = {
      generated_at_ms: Date.UTC(2026, 0, 1),
      summary: {
        total_devices: 1,
        collecting_devices: 1,
        online_devices: 1,
        bus_count: 1,
        queue_len: 0,
        total_reads: 1,
        success_rate: 100,
        disk_level: 'ok',
        disk_free_pct: 50,
        disk_thresholds: { warn: 25, critical: 15, blocking: 10 },
      },
      components: { process: 'ok', mqtt: 'down', queue: 'error' },
      buses: [],
      problems: [],
    }
    state.loading = false
    state.overall = { label: '正常', type: 'success', value: 'ok' }
    state.refresh.mockResolvedValue(undefined)
  })

  it('`error` 档位显红（danger），不是琥珀色 warning', async () => {
    const wrapper = mount(HealthReport)
    await settle()
    const tags = collectTags(wrapper)
    const errorTag = tags.find(([text]) => text.includes('异常'))
    expect(errorTag).toBeTruthy()
    expect(errorTag![1]).toBe('danger')
    wrapper.unmount()
  })

  it('健康报告页与运行总览页的组件中文名 + 徽章色逐项一致', async () => {
    const reportWrapper = mount(HealthReport)
    await settle()
    const statusWrapper = mount(HealthStatus, {
      props: {
        healthRows: [
          { name: 'process', status: 'ok' },
          { name: 'mqtt', status: 'down' },
          { name: 'queue', status: 'error' },
        ],
      },
    })
    await settle()

    const reportTags = collectTags(reportWrapper)
    const statusTags = collectTags(statusWrapper)

    // 两页用的是同一份数据，逐项都能对上
    expect(reportTags.length).toBeGreaterThan(0)
    expect(statusTags.length).toBeGreaterThan(0)
    for (const [, type] of reportTags) {
      expect(['success', 'danger', 'warning', 'info']).toContain(type)
    }
    // 三档在两页的徽章色必须一致（按中文状态名对齐）
    const pick = (tags: Array<[string, string]>, word: string) =>
      tags.find(([text]) => text.includes(word))?.[1]
    for (const word of ['正常', '不可用', '异常']) {
      expect(pick(reportTags, word)).toBe(pick(statusTags, word))
      expect(pick(reportTags, word)).toBeDefined()
    }

    reportWrapper.unmount()
    statusWrapper.unmount()
  })

  it('英文原词不再漏到健康报告页界面', async () => {
    const wrapper = mount(HealthReport)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('网关主进程')
    expect(text).toContain('MQTT 客户端')
    expect(text).not.toContain('process：')
    expect(text).not.toContain('mqtt：')
    wrapper.unmount()
  })
})

/* -------------------------------------------------------------------------- */
/* 2) 综合状态中文名同源（DD-70）                                                */
/* -------------------------------------------------------------------------- */

describe('综合状态中文名同源（DD-70）', () => {
  it('ok / degraded / critical 三档中文名与徽章色只有一份', () => {
    expect(overallStatusMeta('ok')).toEqual({ label: '正常', type: 'success' })
    expect(overallStatusMeta('degraded')).toEqual({ label: '降级', type: 'warning' })
    expect(overallStatusMeta('critical')).toEqual({ label: '严重', type: 'danger' })
  })

  it('未知取值退化为「暂无数据」，不冒充已知档位', () => {
    expect(overallStatusMeta(undefined)).toEqual({ label: '暂无数据', type: 'info' })
    expect(overallStatusMeta('weird')).toEqual({ label: '暂无数据', type: 'info' })
  })

  it('总览异常信号用的中文名与 store 的 overallStatus 一致', async () => {
    const { useHealthSignals } = await import('@/composables/dashboard/useHealthSignals')
    const { ref } = await import('vue')
    const report = ref<Record<string, unknown> | null>({
      status: 'critical',
      problems: [],
      summary: {},
      components: {},
      buses: [],
      generated_at_ms: 0,
    })
    const signals = useHealthSignals(report as never, () => true, () => [])
    await Promise.resolve()
    const statusSignal = signals.value.find((s) => s.id === 'health-status')
    expect(statusSignal).toBeTruthy()
    expect(statusSignal!.title).toContain(overallStatusMeta('critical').label)
  })
})

/* -------------------------------------------------------------------------- */
/* 3) 磁盘水位档位同源（DD-71）                                                  */
/* -------------------------------------------------------------------------- */

describe('磁盘水位档位同源（DD-71）', () => {
  it('四档中文名与徽章色只有一份', () => {
    expect(diskLevelMeta('ok')).toEqual({ label: '正常', type: 'success' })
    expect(diskLevelMeta('warn')).toEqual({ label: '预警', type: 'warning' })
    expect(diskLevelMeta('critical')).toEqual({ label: '严重', type: 'danger' })
    expect(diskLevelMeta('blocking')).toEqual({ label: '阻断', type: 'danger' })
  })

  it('未知档位退化为「正常」而不是 undefined', () => {
    expect(diskLevelMeta('nope')).toEqual({ label: '正常', type: 'success' })
    expect(diskLevelMeta(undefined)).toEqual({ label: '正常', type: 'success' })
  })
})

/* -------------------------------------------------------------------------- */
/* 4) 问题分级同源（DD-72 / DD-69）                                             */
/* -------------------------------------------------------------------------- */

describe('问题分级同源（DD-72 / DD-69）', () => {
  it('三档中文名与徽章色只有一份', () => {
    expect(problemLevelMeta('critical')).toEqual({ label: '严重', type: 'danger' })
    expect(problemLevelMeta('warning')).toEqual({ label: '警告', type: 'warning' })
    expect(problemLevelMeta('info')).toEqual({ label: '提示', type: 'info' })
  })

  it('未知分级退化为「提示」，不冒充严重', () => {
    expect(problemLevelMeta('fatal')).toEqual({ label: '提示', type: 'info' })
  })

  it('分级计数只有一份 reduce，未登记等级不计入', () => {
    expect(
      countProblemLevels([
        { level: 'critical' },
        { level: 'critical' },
        { level: 'warning' },
        { level: 'info' },
        { level: 'fatal' },
      ]),
    ).toEqual({ critical: 2, warning: 1, info: 1 })
    expect(countProblemLevels([])).toEqual({ critical: 0, warning: 0, info: 0 })
    expect(countProblemLevels(null)).toEqual({ critical: 0, warning: 0, info: 0 })
  })

  it('结论条优先用调用方传入的 store 计数，未传时用同一份纯函数自算', async () => {
    const problems = [
      { level: 'critical' as const, code: 'A', message: 'm', hint: 'h' },
      { level: 'warning' as const, code: 'B', message: 'm', hint: 'h' },
    ]
    const status = { value: 'ok' as const, label: '正常', type: 'success' as const }

    // 自算路径
    const self = mount(HealthConclusionBar, { props: { problems, status } })
    await settle()
    expect(self.text()).toContain('严重 1')
    expect(self.text()).toContain('警告 1')
    self.unmount()

    // 传入 store 计数路径（结果必须与自算一致）
    const passed = mount(HealthConclusionBar, {
      props: { problems, status, counts: countProblemLevels(problems) },
    })
    await settle()
    expect(passed.text()).toContain('严重 1')
    expect(passed.text()).toContain('警告 1')
    passed.unmount()
  })
})

/* -------------------------------------------------------------------------- */
/* 5) 组件状态词表本身未被改动（DD-68 的源头）                                    */
/* -------------------------------------------------------------------------- */

describe('healthStatusInfo 仍是组件状态的唯一真源（DD-68）', () => {
  it('`error` 仍是 error 严重度 + danger 徽章', () => {
    expect(healthStatusInfo('error')).toEqual({ label: '异常', severity: 'error', tag: 'danger' })
  })
})
