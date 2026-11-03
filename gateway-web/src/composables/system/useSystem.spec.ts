import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { ElMessage } from 'element-plus'

// 所有外部依赖用 vi.hoisted 提升，便于 vi.mock 工厂引用
const m = vi.hoisted(() => ({
  getStatus: vi.fn(),
  getVersion: vi.fn(),
  getMetrics: vi.fn(),
  healthReady: vi.fn(),
  getHealthReport: vi.fn(),
  parsePrometheusText: vi.fn(),
  getNtpStatus: vi.fn(),
  ntpSync: vi.fn(),
  getConfig: vi.fn(),
  postConfig: vi.fn(),
  confirmAction: vi.fn(),
  notify: vi.fn(),
  setUrl: vi.fn(),
  retryAllDeadLetters: vi.fn(),
  buildDiagnosticReport: vi.fn(),
  downloadDiagnosticReport: vi.fn(),
  buildBackup: vi.fn(),
  downloadBackup: vi.fn(),
  parseBackupFile: vi.fn(),
  recordRead: vi.fn(),
  listDevices: vi.fn(),
  listPoints: vi.fn(),
  readPointNow: vi.fn(),
  listAuditLogs: vi.fn(),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})
vi.mock('@/utils/confirm', () => ({ confirmAction: (o: unknown) => m.confirmAction(o) }))
vi.mock('@/api/status', () => ({
  getStatus: () => m.getStatus(),
  getVersion: () => m.getVersion(),
  getMetrics: () => m.getMetrics(),
  healthReady: () => m.healthReady(),
  getHealthReport: () => m.getHealthReport(),
  parsePrometheusText: (t: string) => m.parsePrometheusText(t),
}))
vi.mock('@/api/network', () => ({
  getNtpStatus: () => m.getNtpStatus(),
  ntpSync: () => m.ntpSync(),
}))
vi.mock('@/api/config', () => ({
  getConfig: () => m.getConfig(),
  postConfig: (c: unknown) => m.postConfig(c),
}))
vi.mock('@/api/devices', () => ({ listDevices: () => m.listDevices() }))
// B3：审计只有后端一条链路，useSystem 通过 listAuditLogs 读取
vi.mock('@/api/auditLog', () => ({ listAuditLogs: (f: unknown) => m.listAuditLogs(f) }))
vi.mock('@/api/points', () => ({
  listPoints: (id: string) => m.listPoints(id),
  readPointNow: (id: string) => m.readPointNow(id),
}))
vi.mock('@/utils/diagnosticReport', () => ({
  buildDiagnosticReport: (i: unknown) => m.buildDiagnosticReport(i),
  downloadDiagnosticReport: (html: string) => m.downloadDiagnosticReport(html),
}))
vi.mock('@/utils/systemBackup', () => ({
  buildBackup: (c: unknown, a: unknown, v?: string) => m.buildBackup(c, a, v),
  downloadBackup: (b: unknown) => m.downloadBackup(b),
  parseBackupFile: (f: unknown) => m.parseBackupFile(f),
}))
vi.mock('@/utils/deviceComms', () => ({
  recordRead: (id: string, ok: boolean, lat: number) => m.recordRead(id, ok, lat),
}))
vi.mock('@/composables/uplink/useWebhook', async () => {
  const { ref } = await import('vue')
  return {
    useWebhook: () => {
      const webhookUrl = ref('')
      const deadLetterQueue = ref<Array<Record<string, unknown>>>([])
      return {
        webhookUrl,
        deadLetterQueue,
        setUrl: m.setUrl,
        isEnabled: () => !!webhookUrl.value,
        notify: m.notify,
        retryAllDeadLetters: m.retryAllDeadLetters,
        clearDeadLetters: vi.fn(),
        retryDeadLetter: vi.fn(),
      }
    },
  }
})

import { createPinia, setActivePinia } from 'pinia'
import { useSystem } from './useSystem'

const mounted: Array<{ unmount: () => void }> = []

function harness() {
  let api: ReturnType<typeof useSystem> | null = null
  const Comp = defineComponent({
    setup() {
      api = useSystem()
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  mounted.push(wrapper)
  return { wrapper, get api() { return api! } }
}

describe('useSystem', () => {
  beforeEach(() => {
    localStorage.clear()
    vi.clearAllMocks()
    // useSystem 内部经 useHealthReport 读取健康快照（D3 已迁入 Pinia）
    setActivePinia(createPinia())
    m.getStatus.mockResolvedValue({ online_devices: 1, queue_length: 2, cpu_usage: 3, mem_usage: 4 })
    m.getVersion.mockResolvedValue({ version: '1.0.0', build_time: '2026-01-01', git_hash: 'abc' })
    m.getMetrics.mockResolvedValue('metric_a 1')
    m.healthReady.mockResolvedValue({ status: 'ok', components: { database: 'ok' } })
    m.getHealthReport.mockResolvedValue({
      generated_at_ms: 1,
      status: 'ok',
      components: { process: 'ok' },
      summary: { total_devices: 0, collecting_devices: 0, online_devices: 0, bus_count: 0, queue_len: 0, total_reads: 0, success_rate: null },
      buses: [],
      problems: [],
    })
    m.parsePrometheusText.mockReturnValue({ metric_a: 1 })
    m.getNtpStatus.mockResolvedValue({ status: 'sync', offset: 5 })
    m.getConfig.mockResolvedValue({ buses: [], devices: [], points: [] })
    m.confirmAction.mockResolvedValue(true)
    m.ntpSync.mockResolvedValue({ status: 'ok' })
    m.listDevices.mockResolvedValue([])
    m.listPoints.mockResolvedValue([])
    m.listAuditLogs.mockResolvedValue([])
    m.readPointNow.mockResolvedValue({ status: 'ok', value: 1 })
    m.buildDiagnosticReport.mockReturnValue('<html/>')
    m.buildBackup.mockReturnValue({ meta: { deviceCount: 0, pointCount: 0 }, config: {} })
    m.parseBackupFile.mockResolvedValue({ meta: { deviceCount: 2, pointCount: 3 }, config: { buses: [] } })
  })

  afterEach(() => {
    mounted.forEach((w) => w.unmount())
    mounted.length = 0
    vi.useRealTimers()
  })

  it('初始状态为默认值，挂载后 loading=true 触发 refresh', () => {
    const { api } = harness()
    expect(api.status).toMatchObject({ online_devices: 0, queue_length: 0, cpu_usage: 0, mem_usage: 0 })
    expect(api.version.version).toBe('-')
    expect(api.ntp.status).toBe('unknown')
    expect(api.metrics.value).toEqual({})
    expect(api.loading.value).toBe(true)
  })

  it('refresh 将各接口结果写入 status/version/metrics/ntp，并解析 Prometheus 文本', async () => {
    const { api } = harness()
    await flushPromises()
    expect(api.status).toMatchObject({ online_devices: 1, queue_length: 2, cpu_usage: 3, mem_usage: 4 })
    expect(api.version.version).toBe('1.0.0')
    expect(api.ntp).toMatchObject({ status: 'sync', offset: 5 })
    expect(api.metrics.value).toEqual({ metric_a: 1 })
    expect(m.parsePrometheusText).toHaveBeenCalledWith('metric_a 1')
    expect(api.loading.value).toBe(false)
  })

  it('refresh 在 getMetrics 返回对象时直接使用对象（不解析文本）', async () => {
    const { api } = harness()
    await flushPromises()
    m.parsePrometheusText.mockClear()
    m.getMetrics.mockResolvedValue({ a: 1 })
    await api.refresh()
    expect(api.metrics.value).toEqual({ a: 1 })
    expect(m.parsePrometheusText).not.toHaveBeenCalled()
  })

  it('metricsText/metricsHtml：JSON 美化 + 转义 + 语法高亮', () => {
    const { api } = harness()
    api.metrics.value = { '<b>': 1, s: 'x', flag: true, none: null }
    expect(api.metricsText.value).toContain('"<b>": 1')
    const html = api.metricsHtml.value
    expect(html).toContain('&lt;b&gt;')
    expect(html).toContain('json-key')
    expect(html).toContain('json-number')
    expect(html).toContain('json-string')
    expect(html).toContain('json-boolean')
    expect(html).toContain('json-null')
  })

  it('fmtPct 与 ntpTagType 映射正确', () => {
    const { api } = harness()
    expect(api.fmtPct(50)).toBe('50.0%')
    expect(api.fmtPct('12.34')).toBe('12.3%')
    expect(api.fmtPct(null)).toBe('-')
    expect(api.fmtPct('abc')).toBe('-')
    expect(api.ntpStatusMap['sync']).toBe('已同步')
    expect(api.ntpTagType('sync')).toBe('success')
    expect(api.ntpTagType('error')).toBe('danger')
    expect(api.ntpTagType('not_sync')).toBe('warning')
    expect(api.ntpTagType('whatever')).toBe('info')
  })

  it('syncNtp 成功提示，失败静默不抛', async () => {
    const { api } = harness()
    await flushPromises()
    await api.syncNtp()
    expect(m.ntpSync).toHaveBeenCalledTimes(1)
    expect(ElMessage.success).toHaveBeenCalledWith('NTP 同步已触发')

    vi.mocked(ElMessage.success).mockClear()
    m.ntpSync.mockRejectedValue(new Error('down'))
    await api.syncNtp()
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('saveWebhook 保存 URL 并根据是否为空提示启用/禁用', async () => {
    const { api } = harness()
    await flushPromises()
    api.webhookUrl.value = '  https://hook/x  '
    api.saveWebhook()
    expect(m.setUrl).toHaveBeenCalledWith('  https://hook/x  ')
    expect(ElMessage.success).toHaveBeenCalledWith('Webhook 已启用')

    api.webhookUrl.value = ''
    api.saveWebhook()
    expect(ElMessage.success).toHaveBeenLastCalledWith('Webhook 已禁用')
  })

  it('testWebhook：无 URL 直接返回；有 URL 时调用 notify；失败提示错误', async () => {
    const { api } = harness()
    await flushPromises()
    await api.testWebhook()
    expect(m.notify).not.toHaveBeenCalled()

    api.webhookUrl.value = 'https://hook/x'
    await api.testWebhook()
    expect(m.notify).toHaveBeenCalledWith('test', expect.any(String))
    expect(ElMessage.success).toHaveBeenCalledWith('测试通知已发送')

    m.notify.mockRejectedValue(new Error('net'))
    await api.testWebhook()
    expect(ElMessage.warning).toHaveBeenCalledWith('发送失败，请检查 URL')
  })

  it('genDiagnostic 采集设备连通性并生成/下载报告', async () => {
    const { api } = harness()
    await flushPromises()
    m.listDevices.mockResolvedValue([{ id: 'd1', name: 'dev1' }])
    m.listPoints.mockResolvedValue([{ id: 'p1' }])
    m.readPointNow.mockResolvedValue({ status: 'ok', value: 1 })

    await api.genDiagnostic()
    expect(api.diagLoading.value).toBe(false)
    const arg = m.buildDiagnosticReport.mock.calls[0]![0] as {
      deviceChecks: unknown[]
      health?: { status: string }
    }
    expect(arg.deviceChecks).toEqual([
      { id: 'd1', name: 'dev1', reachable: true, latency: expect.any(Number) },
    ])
    expect(arg.health).toEqual({ status: 'ok', components: { database: 'ok' } })
    expect(m.recordRead).toHaveBeenCalledWith('d1', true, expect.any(Number))
    expect(m.downloadDiagnosticReport).toHaveBeenCalledWith('<html/>')
    expect(ElMessage.success).toHaveBeenCalledWith('诊断报告已生成并下载')
  })

  it('doBackup 打包配置并下载，记录 lastBackup', async () => {
    const { api } = harness()
    await flushPromises()
    m.getConfig.mockResolvedValue({ buses: [{}], devices: [{}, {}], points: [] })
    m.getVersion.mockResolvedValue({ version: '9.9.9' })
    const backup = { meta: { deviceCount: 2, pointCount: 0 }, config: {} }
    m.buildBackup.mockReturnValue(backup)

    await api.doBackup()
    expect(m.buildBackup).toHaveBeenCalledWith(expect.anything(), expect.anything(), '9.9.9')
    expect(m.downloadBackup).toHaveBeenCalledWith(backup)
    expect(api.lastBackup.value).toEqual(backup)
    expect(api.backupLoading.value).toBe(false)
  })

  it('onRestoreFile：解析失败提示错误，不弹确认', async () => {
    const { api } = harness()
    await flushPromises()
    m.parseBackupFile.mockRejectedValue(new Error('bad json'))
    await api.onRestoreFile({ text: async () => '{}' } as unknown as File)
    expect(ElMessage.error).toHaveBeenCalledWith('bad json')
    expect(m.confirmAction).not.toHaveBeenCalled()
  })

  it('onRestoreFile：确认后导入配置并记录 lastBackup；取消则不导入', async () => {
    const { api } = harness()
    await flushPromises()
    const file = { text: async () => '{}' } as unknown as File

    m.parseBackupFile.mockResolvedValue({ meta: { deviceCount: 2, pointCount: 3 }, config: { buses: [] } })
    m.confirmAction.mockResolvedValue(true)
    m.postConfig.mockResolvedValue({ status: 'ok' })
    await api.onRestoreFile(file)
    expect(m.postConfig).toHaveBeenCalledWith({ buses: [] })
    expect(ElMessage.success).toHaveBeenCalledWith('还原成功，配置已更新')
    expect(api.lastBackup.value?.meta.deviceCount).toBe(2)

    m.confirmAction.mockResolvedValue(false)
    m.postConfig.mockClear()
    await api.onRestoreFile(file)
    expect(m.postConfig).not.toHaveBeenCalled()
  })

  /**
   * 回归：refresh 由 5s 定时器驱动，而 loading 绑在 SystemOverview 根节点的 v-loading 上。
   * 原实现每轮都置 loading=true → 系统总览页每 5 秒被整页蒙层覆盖、期间无法操作。
   * 现改为仅首次加载显示遮罩；上方"挂载后 loading=true"用例守住首屏那一侧。
   */
  it('5s 轮询为静默刷新：首屏显示遮罩，后续轮次不再置 loading', async () => {
    // 只伪造 setInterval，保留真实 setTimeout（flushPromises 依赖它）
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    const { api } = harness()
    expect(api.loading.value).toBe(true) // 首屏：显示整页遮罩
    await flushPromises()
    expect(api.loading.value).toBe(false)

    // 让定时器触发的下一轮请求挂起，便于在轮询期间观察 loading
    let resolveNext!: (v: unknown) => void
    m.getStatus.mockReturnValue(new Promise((r) => { resolveNext = r }))
    vi.advanceTimersByTime(5000)
    await flushPromises()
    // 修复前：此处为 true → 每 5s 整页蒙层闪烁
    expect(api.loading.value).toBe(false)

    resolveNext({ online_devices: 1, queue_length: 2, cpu_usage: 3, mem_usage: 4 })
    await flushPromises()
    expect(api.loading.value).toBe(false)
  })

  /**
   * B3：前端本地操作审计已下线，审计列表唯一来源是后端 /api/audit/logs。
   * 后端只有查询端点，没有删除端点，所以只提供 refreshAudit，不提供 clearAudit。
   */
  it('refreshAudit：审计列表来自后端，limit 200，且不暴露清空能力', async () => {
    const { api } = harness()
    await flushPromises()
    const rows = [
      { ts: 1700000000000, action: 'config.update', target: 'devices', detail: '2 项变更', source: 'ui' },
    ]
    m.listAuditLogs.mockResolvedValue(rows)
    await api.refreshAudit()
    expect(m.listAuditLogs).toHaveBeenCalledWith({ limit: 200 })
    expect(api.auditEntries.value).toEqual(rows)
    expect('clearAudit' in api).toBe(false)
  })

  it('refreshAudit：后端不可达时保留上一次结果，不抛错', async () => {
    const { api } = harness()
    await flushPromises()
    const rows = [{ ts: 1, action: 'a', target: 't', detail: 'd', source: 's' }]
    m.listAuditLogs.mockResolvedValue(rows)
    await api.refreshAudit()
    expect(api.auditEntries.value).toEqual(rows)

    m.listAuditLogs.mockRejectedValue(new Error('down'))
    await api.refreshAudit()
    expect(api.auditEntries.value).toEqual(rows)
  })
})