import { ref, shallowRef, triggerRef, computed, watch, onMounted, onUnmounted, nextTick } from 'vue'
import { ElMessage } from 'element-plus'
import { GatewayWebSocket } from '@/utils/websocket'
import { useWebsocketStore } from '@/stores/websocket'
import { diagnosticsExec } from '@/api/diagnostics'
import { useWebhook } from '@/composables/uplink/useWebhook'
import { precheckCommand, QUICK_DIAGNOSTIC_COMMANDS } from '@/utils/diagnosticsCommands'
import { useVirtualLogs } from './useVirtualLogs'
import { triggerDownload } from '@/utils/exportCsv'

interface LogItem { id: number; text: string }

/** 日志级别按钮配置 */
const LEVEL_BUTTONS = [
  { key: 'all', label: '全部', type: 'info' },
  { key: 'error', label: 'ERROR', type: 'danger' },
  { key: 'warn', label: 'WARN', type: 'warning' },
  { key: 'info', label: 'INFO', type: 'primary' },
  { key: 'debug', label: 'DEBUG', type: 'info' },
]

/** 级别筛选的正则映射（词边界，避免 ERROR 命中 info 日志中的 'error' 字串） */
const LEVEL_REGEX = {
  error: /\b(ERROR|ERR|FATAL)\b/i,
  warn: /\b(WARN|WARNING)\b/i,
  info: /\b(INFO|NOTICE)\b/i,
  debug: /\b(DEBUG|TRACE)\b/i,
}

/** 常用诊断命令快捷按钮
 *  B2 修复：替换为后端白名单子集（见 utils/diagnosticsCommands），下拉与
 *  自由输入共用同一预校验逻辑，避免出现"点了快捷按钮但被后端拒"的 UX 撕裂。
 */
const QUICK_COMMANDS: ReadonlyArray<string> = QUICK_DIAGNOSTIC_COMMANDS

const MAX_LOGS = 1000
const FLUSH_INTERVAL = 200
const BATCH_SIZE = 50
const ALERT_DEBOUNCE_MS = 5000
const AT_BOTTOM_THRESHOLD_PX = 50
/** 超过该行数才启用窗口化渲染（与 DataTable 的 VIRTUAL_THRESHOLD 策略一致） */
const VIRTUAL_THRESHOLD = 200

/**
 * Logs 页面数据层封装
 *
 * 把 Logs.vue 中的日志接收（WebSocket + 缓冲队列 + 级别筛选 + 关键词高亮）、
 * 告警规则、诊断命令（自定义/快捷）、暂停/清空/导出、回到底部、生命周期等
 * 全部集中到一个 composable，主组件只剩 template。
 */
export function useLogs() {
  const wsStore = useWebsocketStore()
  // 连接状态用日志 WS 自身的 onStatusChange 回调（下方 onMounted 绑定），
  // 而非全局 store——store.status 反映的是 /api/realtime 实时流，
  // 与日志端点 /api/diagnostics/logs 的连接态不必然一致。
  const connected = ref(false)
  const reconnectAttempts = ref(0)
  const paused = ref(false)
  // shallowRef：日志项 {id,text} 是只读的、只追加/截断，从不原地修改；
  // 用深层 ref 会让最多 1000 条日志在渲染时逐个生成响应式代理（纯开销）。
  // 数组增删后手动 triggerRef 通知一次，语义与 ref 等价。
  const logs = shallowRef<LogItem[]>([])
  let logIdCounter = 0
  // 虚拟滚动的版本信号：flushLogs 原地 push/splice 后 triggerRef，
  // 但 filteredLogs 无筛选时返回同一个数组引用（W24 优化），
  // Vue computed 对 Object.is 相等的值不触发下游——需要显式版本号驱动虚拟化链路重算
  const logsTick = ref(0)
  const filter = ref('')
  const logBox = ref<HTMLElement>()

  // 告警规则
  const alertKeywords = ref('')
  let alertRules: string[] = []
  const alertCount = ref(0)
  let lastAlertTime = 0

  function updateAlertRules() {
    const oldRules = alertRules
    alertRules = alertKeywords.value
      .split(',')
      .map((s) => s.trim().toLowerCase())
      .filter(Boolean)
    if (JSON.stringify(oldRules) !== JSON.stringify(alertRules)) {
      alertCount.value = 0
    }
  }

  function checkAlert(line: string) {
    if (alertRules.length === 0) return
    const lower = line.toLowerCase()
    const hit = alertRules.find((kw) => lower.includes(kw))
    if (!hit) return
    alertCount.value++
    const now = Date.now()
    if (now - lastAlertTime < ALERT_DEBOUNCE_MS) return
    lastAlertTime = now
    useWebhook().notify('log_alert', line.slice(0, 200), { keyword: hit }).catch(() => { /* ignore */ })
    if ('Notification' in window) {
      if (Notification.permission === 'granted') {
        try {
          new Notification('网关日志告警', { body: line.slice(0, 120), tag: 'gw-log-alert' })
        } catch { /* ignore */ }
      } else if (Notification.permission !== 'denied') {
        Notification.requestPermission().catch(() => { /* ignore */ })
      }
    }
  }

  const command = ref('')
  const cmdOutput = ref('')
  const cmdLoading = ref(false)
  const atBottom = ref(true)
  const activeLevel = ref('all')
  let unmounted = false

  // B2 修复：响应式预校验结果——command 一变即重算，UI 可在用户输入时即时提示。
  const cmdPrecheck = computed(() =>
    command.value.trim() ? precheckCommand(command.value) : null,
  )

  watch(command, () => {
    cmdOutput.value = ''
  })

  const filteredLogs = computed(() => {
    let result = logs.value
    if (activeLevel.value !== 'all') {
      const re = LEVEL_REGEX[activeLevel.value as keyof typeof LEVEL_REGEX]
      if (re) result = result.filter((l) => re.test(l.text))
    }
    if (filter.value) {
      try {
        const re = new RegExp(filter.value, 'i')
        result = result.filter((l) => re.test(l.text))
      } catch {
        result = result.filter((l) => l.text.includes(filter.value))
      }
    }
    return result
  })

  // 窗口化渲染（虚拟滚动）：行数超阈值时只渲染可视窗口 ± overscan，
  // 其余用等高占位撑开滚动条。小日志量全量渲染，零行为差异。
  const virtual = useVirtualLogs({
    source: filteredLogs,
    container: logBox,
    atBottom,
    threshold: VIRTUAL_THRESHOLD,
    version: logsTick,
  })
  const { renderItems, topPad, bottomPad, virtualEnabled } = virtual

  // 窗口内容变化（flush 追加 / 滚动换窗 / 筛选）后实测新行高，
  // 让占位高度逐步从估算值收敛到真实值（measure 内部无变化即短路）
  watch(renderItems, () => {
    void virtual.measure()
  })

  function logLevelClass(line: string) {
    if (LEVEL_REGEX.error.test(line)) return 'log-error'
    if (LEVEL_REGEX.warn.test(line)) return 'log-warn'
    if (LEVEL_REGEX.info.test(line)) return 'log-info'
    if (LEVEL_REGEX.debug.test(line)) return 'log-debug'
    return ''
  }

  // 高亮正则按 filter 记忆编译：旧实现在 v-for 内逐行 new RegExp，
  // 1000 条日志每次渲染要编译 1000 次同一个正则（日志流式刷入时渲染频繁）。
  const highlightRe = computed(() => {
    if (!filter.value) return null
    try {
      return new RegExp(filter.value, 'gi')
    } catch {
      // 非法正则降级为字面量匹配，与 filteredLogs 的降级策略一致
      return new RegExp(filter.value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi')
    }
  })

  function highlightLine(line: string): string {
    const re = highlightRe.value
    if (!re) return escapeHtml(line)
    // String.replace 每次调用都从 0 开始并复位 lastIndex，全局正则可安全复用
    return escapeHtml(line).replace(re, (m) => `<mark class="hl">${m}</mark>`)
  }

  function escapeHtml(s: string): string {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  }

  // 日志缓冲队列：每 FLUSH_INTERVAL / BATCH_SIZE 条触发一次 flush
  const logQueue: LogItem[] = []
  let flushTimer: ReturnType<typeof setTimeout> | null = null

  function flushLogs() {
    if (unmounted) return
    if (flushTimer) {
      clearTimeout(flushTimer)
      flushTimer = null
    }
    if (logQueue.length === 0) return
    // W24: push + splice 局部操作替代整列表赋值，减少 v-for patch 范围
    logs.value.push(...logQueue)
    if (logs.value.length > MAX_LOGS) {
      logs.value.splice(0, logs.value.length - MAX_LOGS)
    }
    // shallowRef 的数组内变更不会自动触发依赖，需显式通知一次
    triggerRef(logs)
    // 同步驱动虚拟化链路（见 logsTick 注释）
    logsTick.value++
    logQueue.length = 0
    if (atBottom.value) {
      nextTick(() => {
        if (logBox.value) {
          logBox.value.scrollTop = logBox.value.scrollHeight
        }
      })
    }
  }

  function onLog(line: string) {
    if (unmounted || paused.value) return
    checkAlert(line)
    logQueue.push({ id: ++logIdCounter, text: line })
    if (logQueue.length >= BATCH_SIZE) {
      flushLogs()
    } else if (!flushTimer) {
      flushTimer = setTimeout(flushLogs, FLUSH_INTERVAL)
    }
  }

  function clearLogs() {
    logs.value = []
    logQueue.length = 0
    atBottom.value = true
    alertCount.value = 0
  }

  function exportLogs() {
    if (logs.value.length === 0) { ElMessage.warning('暂无日志可导出'); return }
    const lines = logs.value.map((l) => l.text)
    const blob = new Blob([lines.join('\n')], { type: 'text/plain;charset=utf-8' })
    triggerDownload(blob, `gateway-logs-${new Date().toISOString().slice(0, 10)}.log`)
    ElMessage.success(`已导出 ${logs.value.length} 条日志`)
  }

  function togglePause() {
    paused.value = !paused.value
  }

  function onScroll() {
    if (!logBox.value) return
    const { scrollTop, scrollHeight, clientHeight } = logBox.value
    atBottom.value = scrollHeight - scrollTop - clientHeight < AT_BOTTOM_THRESHOLD_PX
    // 同步窗口定位信号（启用窗口化时才有副作用）
    virtual.onScroll(logBox.value)
  }

  function scrollToBottom() {
    if (!logBox.value) return
    atBottom.value = true
    nextTick(() => {
      if (logBox.value) logBox.value.scrollTop = logBox.value.scrollHeight
    })
  }

  async function execCommand() {
    if (!command.value.trim()) return
    // B2 修复：前端先做白名单预校验，给用户即时反馈。后端仍会兜底拒非白名单命令。
    const precheck = precheckCommand(command.value)
    if (!precheck.ok) {
      cmdOutput.value = ''
      ElMessage.error(precheck.error ?? '命令校验失败')
      return
    }
    cmdLoading.value = true
    try {
      const resp = await diagnosticsExec(command.value.trim())
      if (unmounted) return
      cmdOutput.value = resp.output
    } catch {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error('命令执行失败，请稍后重试')
    } finally {
      if (!unmounted) cmdLoading.value = false
    }
  }

  function runQuick(c: string) {
    command.value = c
    execCommand()
  }

  async function copyOutput() {
    if (!cmdOutput.value) return
    try {
      await navigator.clipboard.writeText(cmdOutput.value)
      ElMessage.success('已复制到剪贴板')
    } catch {
      ElMessage.warning('复制失败，请手动复制')
    }
  }

  let ws: GatewayWebSocket | null = null

  watch(filter, () => {
    if (atBottom.value) {
      nextTick(() => {
        if (logBox.value) {
          logBox.value.scrollTop = logBox.value.scrollHeight
        }
      })
    }
  })

  onMounted(() => {
    wsStore.subscribe()
    ws = new GatewayWebSocket('/api/diagnostics/logs')
    ws.onLog = onLog
    // 日志页头部「已连接/重连中/未连接」标签的数据源
    ws.onStatusChange = (s, attempts) => {
      connected.value = s === 'connected'
      reconnectAttempts.value = attempts ?? 0
    }
    void ws.connect()
  })

  onUnmounted(() => {
    unmounted = true
    wsStore.unsubscribe()
    if (flushTimer) {
      clearTimeout(flushTimer)
      flushTimer = null
    }
    ws?.close()
  })

  return {
    // state
    connected, reconnectAttempts, paused, logs, filter, logBox,
    alertKeywords, alertCount, command, cmdOutput, cmdLoading, atBottom, activeLevel,
    // computed
    filteredLogs, cmdPrecheck,
    // 虚拟滚动（行数 ≤ 阈值时 renderItems === filteredLogs、pads 为 0）
    renderItems, topPad, bottomPad, virtualEnabled,
    // constants
    levels: LEVEL_BUTTONS, quickCommands: QUICK_COMMANDS,
    // helpers
    logLevelClass, highlightLine, escapeHtml,
    // actions
    updateAlertRules, clearLogs, exportLogs, togglePause,
    onScroll, scrollToBottom, execCommand, runQuick, copyOutput,
  }
}
