import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

const m = vi.hoisted(() => ({
  refreshAudit: vi.fn(),
  saveWebhook: vi.fn(),
  testWebhook: vi.fn(),
  retryAllNotify: vi.fn(),
  revokeSession: vi.fn(),
  syncNtp: vi.fn(),
  genDiagnostic: vi.fn(),
  doBackup: vi.fn(),
  onRestoreFile: vi.fn(),
  clearNotifyQueue: vi.fn(),
  retryNotify: vi.fn(),
  // 设备端死信 API
  listDeadLetters: vi.fn(),
  replayDeadLetter: vi.fn(),
  purgeDeadLetters: vi.fn(),
  confirmAction: vi.fn(),
  push: vi.fn(),
  // 可变夹具：部分用例按需覆盖
  webhookUrl: '',
  sessions: [] as unknown[],
  dlq: [] as unknown[],
  lastBackup: null as unknown,
  // 审计行夹具：来自后端 /api/audit/logs 的字段 ts/action/target/detail/source
  auditEntries: [] as unknown[],
  // 健康聚合报告夹具（组件明细 / 问题清单）
  healthComponents: {} as Record<string, string>,
  healthProblems: [] as Array<Record<string, string>>,
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: m.push }),
  useRoute: () => ({ query: {}, params: {} }),
}))

vi.mock('@/api/deadLetter', () => ({
  listDeadLetters: (o: unknown) => m.listDeadLetters(o),
  replayDeadLetter: (id: number) => m.replayDeadLetter(id),
  purgeDeadLetters: (olderThanMs?: number) => m.purgeDeadLetters(olderThanMs),
}))

vi.mock('@/utils/confirm', () => ({ confirmAction: (o: unknown) => m.confirmAction(o) }))

vi.mock('@/composables/system/useSystem', async () => {
  const { ref, reactive, computed } = await import('vue')
  return {
    useSystem: () => ({
      auditEntries: ref(m.auditEntries),
      refreshAudit: m.refreshAudit,
      webhook: {
        isEnabled: () => !!m.webhookUrl,
        notifyRetryQueue: ref(m.dlq),
        clearNotifyQueue: m.clearNotifyQueue,
        retryNotify: m.retryNotify,
      },
      webhookUrl: ref(m.webhookUrl),
      saveWebhook: m.saveWebhook,
      testWebhook: m.testWebhook,
      retryAllNotify: m.retryAllNotify,
      sessionList: ref(m.sessions),
      revokeSession: m.revokeSession,
      status: reactive({ online_devices: 1, queue_length: 2, cpu_usage: 3, mem_usage: 4 }),
      version: reactive({ version: '1.0.0', build_time: '2026-01-01', git_hash: 'abc' }),
      ntp: reactive({ status: 'sync', offset_ms: 12 }),
      loading: ref(false),
      // 组件健康 / 问题清单：来自 /api/health/report 的共享快照
      healthRows: computed(() =>
        Object.entries(m.healthComponents).map(([name, status]) => ({ name, status })),
      ),
      healthProblems: computed(() => m.healthProblems),
      healthLoading: ref(false),
      // 综合状态：与 useHealthReport 的 overallStatus 同一份文案（正常/降级/严重）
      healthOverallStatus: computed(() => ({ value: 'ok' as const, label: '正常', type: 'success' as const })),
      ntpStatusMap: { sync: '已同步', not_sync: '未同步', unknown: '未知', error: '异常' },
      ntpTagType: (s: string) => (s === 'sync' ? 'success' : s === 'error' ? 'danger' : s === 'not_sync' ? 'warning' : 'info'),
      nowTime: ref('2026-10-13 10:00:00'),
      metricsHtml: ref('<span class="json-key">"ok"</span>'),
      fmtPct: (v: unknown) => (v == null ? '-' : `${Number(v).toFixed(1)}%`),
      syncNtp: m.syncNtp,
      diagLoading: ref(false),
      genDiagnostic: m.genDiagnostic,
      backupLoading: ref(false),
      lastBackup: ref(m.lastBackup),
      doBackup: m.doBackup,
      onRestoreFile: m.onRestoreFile,
    }),
  }
})

import SystemOverview from './SystemOverview.vue'

async function gotoTab(wrapper: ReturnType<typeof mount>, index: number) {
  const items = wrapper.findAll('.el-tabs__item')
  await items[index]!.trigger('click')
  await flushPromises()
  await nextTick()
}

describe('SystemOverview.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    m.webhookUrl = ''
    m.sessions = []
    m.dlq = []
    m.lastBackup = null
    m.auditEntries = []
    m.healthComponents = {}
    m.healthProblems = []
    m.listDeadLetters.mockResolvedValue({ protocol: 'http', total: 0, items: [] })
    m.replayDeadLetter.mockResolvedValue({ id: 1, outbox_id: 88, requeued: true })
    m.purgeDeadLetters.mockResolvedValue({ deleted: 0 })
    m.confirmAction.mockResolvedValue(true)
  })

  it('默认"运行状态"tab 渲染系统信息 / NTP / 运行指标', async () => {
    const wrapper = mount(SystemOverview)
    await flushPromises()
    const text = wrapper.text()
    expect(text).toContain('1.0.0')
    expect(text).toContain('2026-01-01')
    expect(text).toContain('abc')
    expect(text).toContain('3.0%') // cpu_usage
    expect(text).toContain('4.0%') // mem_usage
    expect(text).toContain('已同步') // ntpStatusMap['sync']
    expect(text).toContain('12 ms') // ntp.offset_ms
    expect(wrapper.find('.metrics').html()).toContain('json-key')
    wrapper.unmount()
  })

  it('"运行状态"tab：组件健康表数据源是聚合健康报告快照', async () => {
    m.healthComponents = { mqtt: 'down', process: 'ok' }
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await nextTick()
    const text = wrapper.text()
    expect(text).toContain('组件健康状态')
    expect(text).toContain('MQTT 客户端')
    expect(text).toContain('不可用')
    expect(text).toContain('网关主进程')
    expect(text).toContain('正常')
    wrapper.unmount()
  })

  it('"运行状态"tab：问题清单与健康报告同一份 problems', async () => {
    m.healthProblems = [
      { level: 'critical', code: 'MQTT_DISCONNECTED', message: 'MQTT 未连接：采集到的数据无法上传', hint: '检查 broker 地址与凭据' },
    ]
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await nextTick()
    const text = wrapper.text()
    expect(text).toContain('健康问题（1）')
    expect(text).toContain('MQTT_DISCONNECTED')
    expect(text).toContain('检查 broker 地址与凭据')
    wrapper.unmount()
  })

  it('没有健康快照时组件健康表显示空态，不报错', async () => {
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await nextTick()
    expect(wrapper.text()).toContain('暂无组件状态数据')
    wrapper.unmount()
  })

  it('点击"立即同步"触发 syncNtp', async () => {
    const wrapper = mount(SystemOverview)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('立即同步'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(m.syncNtp).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('"通知与会话"tab：Webhook 表单与保存行为，URL 为空时测试按钮禁用', async () => {
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 1)
    const text = wrapper.text()
    expect(text).toContain('变更通知（Webhook）')
    expect(text).toContain('未启用')

    const saveBtn = wrapper.findAll('button').find((b) => b.text().includes('保存'))
    const testBtn = wrapper.findAll('button').find((b) => b.text().includes('测试'))
    expect(testBtn!.attributes('disabled')).toBeDefined()
    await saveBtn!.trigger('click')
    expect(m.saveWebhook).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('"通知与会话"tab：上行转发监控只保留入口，本机通知队列可全部重发', async () => {
    m.dlq = [{ id: 'q1', payload: { event: 'cfg_saved', message: 'msg' }, retryCount: 1, lastError: 'err' }]
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 1)
    const text = wrapper.text()
    expect(text).toContain('上行转发监控')
    expect(text).toContain('上行服务状态')
    // 原「Webhook 死信队列」已更名为本机通知重试队列，并标注仅本机
    expect(text).toContain('通知重试队列（本机浏览器）')
    expect(text).toContain('仅本机')

    const goBtn = wrapper.findAll('button').find((b) => b.text().includes('打开上行服务状态'))
    expect(goBtn).toBeTruthy()
    await goBtn!.trigger('click')
    expect(m.push).toHaveBeenCalledWith('/uplink?tab=north-status')

    const retryAll = wrapper.findAll('button').find((b) => b.text().includes('全部重发'))
    expect(retryAll!.attributes('disabled')).toBeUndefined()
    await retryAll!.trigger('click')
    expect(m.retryAllNotify).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('"通知与会话"tab：设备端死信展示真实未送达数据，支持重投与确认后清理', async () => {
    m.listDeadLetters.mockResolvedValue({
      protocol: 'http',
      total: 2,
      items: [{
        id: 7,
        protocol: 'http',
        data_id: 101,
        topic: 'http://x',
        payload_len: 512,
        payload_preview: '{"a":1}',
        qos: 1,
        created_at: 1700000000000,
        failed_at: 1700000000000,
        attempt_count: 5,
        reason: 'http_404',
        replayed_at: null,
      }],
    })
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 1)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('设备端死信（未送达数据）')
    expect(text).toContain('2 条')
    expect(text).toContain('http_404')
    expect(text).toContain('{"a":1}')
    expect(m.listDeadLetters).toHaveBeenCalledWith({ limit: 50, offset: 0 })

    const replayBtn = wrapper.findAll('button').find((b) => b.text() === '重投')
    expect(replayBtn).toBeTruthy()
    await replayBtn!.trigger('click')
    await flushPromises()
    expect(m.replayDeadLetter).toHaveBeenCalledWith(7)

    const purgeBtn = wrapper.findAll('button').find((b) => b.text().includes('清理'))
    await purgeBtn!.trigger('click')
    await flushPromises()
    expect(m.confirmAction).toHaveBeenCalledTimes(1)
    expect(m.purgeDeadLetters).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('"通知与会话"tab：设备端死信为空时显示「无未送达数据」', async () => {
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 1)
    await flushPromises()
    expect(wrapper.text()).toContain('无未送达数据')
    wrapper.unmount()
  })

  it('"通知与会话"tab：死信库 503 时显示「无法确认」，不能渲染成 0 条', async () => {
    m.listDeadLetters.mockRejectedValue({ response: { status: 503 } })
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 1)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('死信库不可用，无法确认是否存在未送达数据')
    expect(text).toContain('状态未知')
    // 关键：503 不能退化成空表/0 条
    expect(text).not.toContain('无未送达数据')
    wrapper.unmount()
  })

  it('"通知与会话"tab：会话列表渲染并支持强制下线', async () => {
    m.sessions = [
      { id: 's2', loginAt: 1700000000000, lastActiveAt: 1700000000000, ip: '1.2.3.4', userAgent: 'UA-test', current: false },
    ]
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 1)
    const text = wrapper.text()
    expect(text).toContain('登录会话管理')
    expect(text).toContain('1.2.3.4')

    const revoke = wrapper.findAll('button').find((b) => b.text().includes('强制下线'))
    expect(revoke).toBeTruthy()
    await revoke!.trigger('click')
    expect(m.revokeSession).toHaveBeenCalledWith('s2')
    wrapper.unmount()
  })

  it('"审计与维护"tab：触发诊断报告与备份', async () => {
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 2)

    const diag = wrapper.findAll('button').find((b) => b.text().includes('生成并下载诊断报告'))
    const backup = wrapper.findAll('button').find((b) => b.text().includes('生成并下载备份'))
    expect(diag).toBeTruthy()
    expect(backup).toBeTruthy()
    await diag!.trigger('click')
    expect(m.genDiagnostic).toHaveBeenCalledTimes(1)
    await backup!.trigger('click')
    expect(m.doBackup).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('"审计与维护"tab：审计表直接展示后端字段，只有刷新没有清空', async () => {
    m.auditEntries = [
      { ts: 1700000000000, action: 'config.update', target: 'devices', detail: '2 项变更', source: 'ui' },
    ]
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 2)

    const auditCard = wrapper.findAll('.section-card').find((c) => c.text().includes('操作审计日志'))
    expect(auditCard).toBeTruthy()
    const text = auditCard!.text()
    expect(text).toContain('config.update')
    expect(text).toContain('devices')
    expect(text).toContain('2 项变更')
    expect(text).toContain('ui')
    expect(text).toContain('1 条')
    // 后端无删除端点：不提供「清空」
    expect(text).not.toContain('清空')

    const refresh = auditCard!.findAll('button').find((b) => b.text().includes('刷新'))
    expect(refresh).toBeTruthy()
    await refresh!.trigger('click')
    expect(m.refreshAudit).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('"审计与维护"tab：已有备份时渲染备份摘要', async () => {
    m.lastBackup = {
      createdAt: 1700000000000,
      gatewayVersion: '9.9.9',
      meta: { busCount: 1, deviceCount: 2, pointCount: 3 },
      auditLogs: [1, 2],
    }
    const wrapper = mount(SystemOverview)
    await flushPromises()
    await gotoTab(wrapper, 2)
    const text = wrapper.text()
    expect(text).toContain('9.9.9')
    expect(text).toContain('备份时间')
    wrapper.unmount()
  })
})