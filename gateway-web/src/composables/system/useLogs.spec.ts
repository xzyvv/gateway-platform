import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { ElMessage } from 'element-plus'

interface FakeWs {
  onLog: ((line: string) => void) | null
  connect: () => void
  close: () => void
}

const m = vi.hoisted(() => ({
  diagnosticsExec: vi.fn(),
  notify: vi.fn(),
}))

const wsHolder = vi.hoisted(() => ({ last: null as FakeWs | null }))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})
vi.mock('@/api/diagnostics', () => ({
  diagnosticsExec: (cmd: string) => m.diagnosticsExec(cmd),
}))
vi.mock('@/composables/uplink/useWebhook', () => ({
  useWebhook: () => ({ notify: m.notify }),
}))
vi.mock('@/utils/websocket', () => ({
  GatewayWebSocket: class {
    onLog: ((line: string) => void) | null = null
    onMessage = null
    onOpen = null
    onClose = null
    onError = null
    onReconnect = null
    onStatusChange = null
    onHeartbeatTimeout = null
    connect = vi.fn()
    close = vi.fn()
    constructor() {
      wsHolder.last = this
    }
  },
}))

import { useLogs } from './useLogs'
import { useWebsocketStore } from '@/stores/websocket'

type Cli = ReturnType<typeof useLogs>

const mounted: Array<{ unmount: () => void }> = []

function harness() {
  let api: Cli | null = null
  const Comp = defineComponent({
    setup() {
      api = useLogs()
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  mounted.push(wrapper)
  return { wrapper, get api() { return api! } }
}

/** 用假定时器 + 逐条投递日志，advance 后确保缓冲队列 flush 到 logs */
function feed(api: Cli, lines: string[]) {
  for (const l of lines) wsHolder.last!.onLog!(l)
  vi.advanceTimersByTime(200)
  return api
}

describe('useLogs', () => {
  let clickSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.clearAllMocks()
    m.notify.mockResolvedValue(undefined)
    clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
  })

  afterEach(() => {
    mounted.forEach((w) => w.unmount())
    mounted.length = 0
    vi.useRealTimers()
    clickSpy.mockRestore()
  })

  it('初始状态与常量：日志/筛选/告警为空，级别与快捷命令就绪', () => {
    const { api } = harness()
    expect(api.logs.value).toEqual([])
    expect(api.paused.value).toBe(false)
    expect(api.filter.value).toBe('')
    expect(api.activeLevel.value).toBe('all')
    expect(api.alertCount.value).toBe(0)
    expect(api.connected.value).toBe(false)
    expect(api.levels).toHaveLength(5)
    expect(api.quickCommands).toContain('df -h')
  })

  it('单条日志进入缓冲队列，200ms 后 flush', () => {
    vi.useFakeTimers()
    const { api } = harness()
    wsHolder.last!.onLog!('hello world')
    expect(api.logs.value).toHaveLength(0)
    vi.advanceTimersByTime(200)
    expect(api.logs.value).toHaveLength(1)
    expect(api.logs.value[0]!.text).toBe('hello world')
  })

  it('累计到 BATCH_SIZE=50 立即 flush，超过 MAX_LOGS 截断', () => {
    const { api } = harness()
    for (let i = 0; i < 50; i++) wsHolder.last!.onLog!(`line ${i}`)
    expect(api.logs.value).toHaveLength(50)

    for (let i = 50; i < 1100; i++) wsHolder.last!.onLog!(`line ${i}`)
    expect(api.logs.value).toHaveLength(1000)
    expect(api.logs.value[0]!.text).toBe('line 100')
  })

  /** 同步投递：每 50 条触发 BATCH_SIZE 立即 flush，不依赖定时器 */
  function feedSync(api: Cli, n: number, prefix = 'row') {
    for (let i = 0; i < n; i++) wsHolder.last!.onLog!(`${prefix} ${i}`)
    return api
  }

  it('窗口化渲染：行数超阈值才启用，钉底时渲染尾部窗口', () => {
    const { api } = harness()
    // 阈值内：全量渲染，pads 为 0
    feedSync(api, 50, 'small')
    expect(api.virtualEnabled.value).toBe(false)
    expect(api.renderItems.value).toHaveLength(50)
    expect(api.topPad.value).toBe(0)
    expect(api.bottomPad.value).toBe(0)

    // 超阈值（>200）：启用窗口化；atBottom=true 走 tail 窗口，bottomPad 为 0
    feedSync(api, 300, 'bulk')
    expect(api.virtualEnabled.value).toBe(true)
    expect(api.renderItems.value.length).toBeLessThan(api.filteredLogs.value.length)
    expect(api.topPad.value).toBeGreaterThan(0)
    expect(api.bottomPad.value).toBe(0)
    // 窗口必含最新一条
    expect(api.renderItems.value.at(-1)?.id).toBe(api.filteredLogs.value.at(-1)?.id)
  })

  it('窗口化渲染：向上滚动后按 scrollTop 取窗，bottomPad 出现', () => {
    const { api } = harness()
    feedSync(api, 300)
    expect(api.virtualEnabled.value).toBe(true)
    // 模拟滚到中部：logBox 由测试桩提供（onScroll 从中读定位信号）；
    // querySelectorAll 返回空，让异步 measure 安全短路（jsdom 无真实布局）
    api.logBox.value = {
      scrollTop: 100 * 22,
      scrollHeight: 300 * 22,
      clientHeight: 220,
      querySelectorAll: () => [],
    } as unknown as HTMLElement
    api.onScroll()
    expect(api.atBottom.value).toBe(false)
    expect(api.topPad.value).toBeGreaterThan(0)
    expect(api.bottomPad.value).toBeGreaterThan(0)
    // 窗口覆盖 scrollTop 对应行附近，而非尾部
    const first = api.renderItems.value[0]
    expect(first).toBeDefined()
    expect(filteredIndexOf(api, first!.id)).toBeGreaterThan(60)
  })

  /** 日志 id 从 1 递增，filteredLogs 未启用筛选时 id-1 即下标 */
  function filteredIndexOf(api: Cli, id: number): number {
    return api.filteredLogs.value.findIndex((l) => l.id === id)
  }

  it('按级别筛选日志（词边界匹配）', () => {
    vi.useFakeTimers()
    const { api } = harness()
    feed(api, ['ERROR boom', 'INFO ok', 'WARN me', 'DEBUG trace', 'plain'])
    api.activeLevel.value = 'error'
    expect(api.filteredLogs.value.map((l) => l.text)).toEqual(['ERROR boom'])
    api.activeLevel.value = 'warn'
    expect(api.filteredLogs.value.map((l) => l.text)).toEqual(['WARN me'])
    api.activeLevel.value = 'all'
    expect(api.filteredLogs.value).toHaveLength(5)
  })

  it('关键词筛选：正则匹配；非法正则回退 includes；高亮转义', () => {
    vi.useFakeTimers()
    const { api } = harness()
    feed(api, ['alpha boom', '<script>', 'gamma'])
    api.filter.value = 'boom'
    expect(api.filteredLogs.value.map((l) => l.text)).toEqual(['alpha boom'])
    api.filter.value = '['
    expect(api.filteredLogs.value).toHaveLength(0)

    api.filter.value = ''
    expect(api.highlightLine('a<b>&c')).toBe('a&lt;b&gt;&amp;c')
    api.filter.value = 'boom'
    expect(api.highlightLine('x boom y')).toContain('<mark class="hl">boom</mark>')
  })

  it('logLevelClass 按关键字给出样式类', () => {
    const { api } = harness()
    expect(api.logLevelClass('ERROR x')).toBe('log-error')
    expect(api.logLevelClass('WARN y')).toBe('log-warn')
    expect(api.logLevelClass('INFO z')).toBe('log-info')
    expect(api.logLevelClass('DEBUG d')).toBe('log-debug')
    expect(api.logLevelClass('hello')).toBe('')
  })

  it('clearLogs 清空日志与告警计数', () => {
    vi.useFakeTimers()
    const { api } = harness()
    api.alertKeywords.value = 'error'
    api.updateAlertRules()
    feed(api, ['ERROR x'])
    expect(api.alertCount.value).toBe(1)
    api.clearLogs()
    expect(api.logs.value).toEqual([])
    expect(api.alertCount.value).toBe(0)
    expect(api.atBottom.value).toBe(true)
  })

  it('滚动：onScroll 判定是否在底部，scrollToBottom 复位', async () => {
    const { api } = harness()
    api.logBox.value = { scrollTop: 900, scrollHeight: 1000, clientHeight: 100 } as unknown as HTMLElement
    api.onScroll()
    expect(api.atBottom.value).toBe(true)
    api.logBox.value.scrollTop = 100
    api.onScroll()
    expect(api.atBottom.value).toBe(false)
    api.scrollToBottom()
    await nextTick()
    expect(api.atBottom.value).toBe(true)
    expect(api.logBox.value?.scrollTop).toBe(1000)
  })

  it('告警规则：命中关键词计数并按 5s 去抖通知 webhook', () => {
    vi.useFakeTimers()
    const { api } = harness()
    api.alertKeywords.value = 'error, fail'
    api.updateAlertRules()
    wsHolder.last!.onLog!('an ERROR happened')
    expect(api.alertCount.value).toBe(1)
    expect(m.notify).toHaveBeenCalledWith('log_alert', 'an ERROR happened', { keyword: 'error' })
    wsHolder.last!.onLog!('fail again')
    expect(api.alertCount.value).toBe(2)
    expect(m.notify).toHaveBeenCalledTimes(1)
  })

  it('暂停后不再接收日志', () => {
    const { api } = harness()
    api.togglePause()
    expect(api.paused.value).toBe(true)
    wsHolder.last!.onLog!('ignored')
    expect(api.logs.value).toHaveLength(0)
  })

  it('execCommand / runQuick 调用诊断命令接口', async () => {
    const { api } = harness()
    m.diagnosticsExec.mockResolvedValue({ output: 'out' })
    await api.execCommand()
    expect(m.diagnosticsExec).not.toHaveBeenCalled()

    // B2 修复：ls 不在白名单（前端 precheck 拦截，不再打到后端）
    api.command.value = 'ls -l'
    await api.execCommand()
    expect(m.diagnosticsExec).not.toHaveBeenCalled()
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('白名单'))

    // 改用白名单内的命令
    api.command.value = 'df -h'
    await api.execCommand()
    expect(m.diagnosticsExec).toHaveBeenCalledWith('df -h')
    expect(api.cmdOutput.value).toBe('out')
    expect(api.cmdLoading.value).toBe(false)

    api.runQuick('uptime')
    expect(api.command.value).toBe('uptime')
    await flushPromises()
    expect(m.diagnosticsExec).toHaveBeenCalledWith('uptime')
  })

  it('exportLogs / copyOutput 空态与正常路径', async () => {
    vi.useFakeTimers()
    const { api } = harness()
    api.exportLogs()
    expect(ElMessage.warning).toHaveBeenCalledWith('暂无日志可导出')

    wsHolder.last!.onLog!('a line')
    vi.advanceTimersByTime(200)
    api.exportLogs()
    expect(ElMessage.success).toHaveBeenCalledWith('已导出 1 条日志')

    api.cmdOutput.value = 'result'
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    })
    await api.copyOutput()
    expect(ElMessage.success).toHaveBeenCalledWith('已复制到剪贴板')
  })

  it('挂载订阅 / 卸载取消订阅并关闭连接', () => {
    const { wrapper } = harness()
    const store = useWebsocketStore()
    expect(store.subscribers).toBe(1)
    expect(wsHolder.last!.connect).toHaveBeenCalled()
    wrapper.unmount()
    expect(store.subscribers).toBe(0)
    expect(wsHolder.last!.close).toHaveBeenCalled()
    const i = mounted.indexOf(wrapper)
    if (i >= 0) mounted.splice(i, 1)
  })
})