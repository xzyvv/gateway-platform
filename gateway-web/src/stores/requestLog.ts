import { defineStore } from 'pinia'
import { shallowRef, triggerRef, computed } from 'vue'

/**
 * 单条请求记录
 *
 * method/url/status/duration/timestamp 为规范要求字段；
 * startTime/endTime/requestData/responseData/id 保留供请求监视器面板展示。
 */
export interface RequestLogEntry {
  id: number
  method: string
  url: string
  status: number | string
  /** 耗时（毫秒） */
  duration: number
  /** 请求发起时间戳（ms），与 startTime 一致 */
  timestamp: number
  startTime: number
  endTime: number
  requestData?: unknown
  responseData?: unknown
  error?: string
  /** 后端 trace ID（响应头 x-request-id），报障时贴给研发即可定位到日志 */
  traceId?: string
  /**
   * 实际发出的请求头（敏感值已脱敏）。
   *
   * 修复（P1-12）：原先不记录任何请求头，导致
   *   - 导出的 HAR 里 `headers: []` 恒空，拖进 DevTools 无法回放；
   *   - 「复制为 cURL」只能给方法+URL+Body，带鉴权的接口在终端无法重放。
   * 记录头之后二者都能真正复现请求。
   */
  requestHeaders?: Array<{ name: string; value: string }>
  /** 实际收到的响应头（敏感值已脱敏） */
  responseHeaders?: Array<{ name: string; value: string }>
  /** 记录来源：http（默认）或 ws 帧 */
  kind?: 'http' | 'ws'
  /** WS 帧明细（kind === 'ws' 时存在） */
  ws?: {
    direction: 'in' | 'out'
    type: string
    size: number
    data: string
  }
}

/** 向后兼容别名 */
export type RequestRecord = RequestLogEntry

/**
 * 全局请求日志 Store
 *
 * 由 http.ts 拦截器写入，请求监视器面板读取。
 * 限制最多 200 条，超出按 FIFO 淘汰（unshift 新条目，尾部裁剪）。
 */
export const useRequestLogStore = defineStore('requestLog', () => {
  const MAX_LOGS = 200
  // WS 帧频率远高于 HTTP（实时推送 1~5Hz），单独限流，避免几分钟就把 HTTP 记录挤空。
  const MAX_WS_LOGS = 60
  // #15: 单条记录中 requestData/responseData 的体积上限（字符数）。
  // 旧实现只限条数（200）不限字节，一个含大数组/证书/大响应的记录可占用数十 MB 内存，
  // 200 条累计可能导致标签页内存暴涨。超出则截断并标注。
  const MAX_DATA_CHARS = 32 * 1024

  /** 对日志数据做体积截断（连同循环引用等序列化失败场景一并兜底） */
  function truncateData(v: unknown): unknown {
    if (v === undefined || v === null) return v
    try {
      const s = JSON.stringify(v)
      if (typeof s === 'string' && s.length > MAX_DATA_CHARS) {
        return `[truncated: ${s.length} chars > ${MAX_DATA_CHARS}] ${s.slice(0, MAX_DATA_CHARS)}`
      }
    } catch {
      // 循环引用 / BigInt 等无法序列化：原样返回，避免抛错打断日志
    }
    return v
  }

  // shallowRef：RequestLogEntry 的 requestData / responseData 是任意深度的请求/响应体，
  // 深层 ref 会为每层嵌套生成响应式代理——而这是每个 HTTP 请求（含轮询）都写一条的
  // 高频路径，单条 responseData 截断上限 32KB，代理树规模不可忽略。
  // 条目写入后只读（面板仅展示，不做原地修改），故只需通知一次数组变更。
  const logs = shallowRef<RequestLogEntry[]>([])
  const paused = shallowRef(false)
  // 采样开关：生产环境默认关闭（悬浮球与记录都不常驻），由请求监视器打开时临时开启。
  // 关闭时 pushLog 在序列化之前直接返回，避免每个轮询请求都做一次 32KB 级 JSON.stringify。
  const enabled = shallowRef(import.meta.env.DEV)
  let seq = 0

  /** 压入一条请求记录；超过上限时丢弃最旧的；暂停或未启用时忽略 */
  function pushLog(record: Omit<RequestLogEntry, 'id' | 'timestamp'> & { timestamp?: number }) {
    if (paused.value || !enabled.value) return
    const entry: RequestLogEntry = {
      ...record,
      requestData: truncateData(record.requestData),
      responseData: truncateData(record.responseData),
      id: ++seq,
      timestamp: record.timestamp ?? record.startTime,
    }
    logs.value.unshift(entry)
    if (logs.value.length > MAX_LOGS) {
      logs.value.length = MAX_LOGS
    }
    if (entry.kind === 'ws') {
      let wsCount = 0
      for (const r of logs.value) {
        if (r.kind === 'ws') wsCount++
      }
      // 从尾部（最旧）淘汰，保留最新的 MAX_WS_LOGS 条 WS 帧
      for (let i = logs.value.length - 1; i >= 0 && wsCount > MAX_WS_LOGS; i--) {
        if (logs.value[i]?.kind === 'ws') {
          logs.value.splice(i, 1)
          wsCount--
        }
      }
    }
    // shallowRef 的数组内变更不会自动触发依赖，需显式通知一次
    triggerRef(logs)
  }

  /** 开启/关闭请求采样（面板关闭时停采，避免长期占用内存） */
  function setEnabled(v: boolean) {
    enabled.value = v
  }

  /** 清空日志 */
  function clearLogs() {
    logs.value = []
  }

  /** 同步获取当前日志数组（引用同 logs，便于非响应式场景读取） */
  function getLogs(): RequestLogEntry[] {
    return logs.value
  }

  const total = computed(() => logs.value.length)
  // WS 帧不计入错误与均耗：它们的 status 是协议占位值，duration 恒为 0，混入会稀释统计。
  const errors = computed(() => logs.value.filter((r) => r.kind !== 'ws' && (typeof r.status === 'string' || r.status >= 400)).length)
  const avgDuration = computed(() => {
    const httpLogs = logs.value.filter((r) => r.kind !== 'ws')
    if (httpLogs.length === 0) return 0
    const sum = httpLogs.reduce((acc, r) => acc + r.duration, 0)
    return Math.round(sum / httpLogs.length)
  })

  return {
    logs,
    paused,
    enabled,
    setEnabled,
    pushLog,
    clearLogs,
    getLogs,
    total,
    errors,
    avgDuration,
  }
})