/**
 * 通信日志归档（网关真实日志文件）
 *
 * 数据来自后端 `GET /api/diagnostics/logs/archives`：
 * 后端扫描日志目录（`GATEWAY_LOG_DIR`，默认 `/var/log/gateway`），
 * 按 `gateway.log.<YYYY-MM-DD>` 的滚动命名列出真实存在的文件。
 * 早期实现用 localStorage 造 7 条空记录，现场排障时无法确认网关留了哪几天的日志。
 */
import { request } from './index'
import { triggerDownload } from '@/utils/exportCsv'

/** 归档文件条目（与后端 `LogArchiveInfo` 对齐） */
export interface LogArchiveItem {
  /** `YYYY-MM-DD`；当期文件按修改时间推断 */
  date: string
  size_bytes: number
  /** 是否为当期正在写入的文件 */
  current: boolean
}

/** 单条日志（JSON 日志行解析结果；解析失败时只有 raw） */
export interface ArchiveLogLine {
  time: string
  level: string
  target: string
  message: string
  /** 原始行（JSON 解析失败时整行原样展示） */
  raw: string
}

export interface ArchiveView {
  date: string
  lines: ArchiveLogLine[]
}

/** 列出真实存在的日志归档文件（新日期在前） */
export async function listArchives(): Promise<LogArchiveItem[]> {
  const r = await request<{ archives: LogArchiveItem[]; count: number }>({
    url: '/api/diagnostics/logs/archives',
    method: 'GET',
  })
  return r?.archives ?? []
}

/** 把一行文本按 JSON 日志格式解析；非 JSON 行原样保留 */
function parseLine(raw: string): ArchiveLogLine {
  try {
    const j = JSON.parse(raw) as {
      timestamp?: string
      level?: string
      target?: string
      fields?: { message?: unknown; [k: string]: unknown }
    }
    const message =
      (typeof j.fields?.message === 'string' ? j.fields.message : '') ||
      (j.fields ? JSON.stringify(j.fields) : '') ||
      raw
    return {
      time: j.timestamp ?? '',
      level: (j.level ?? '').toUpperCase(),
      target: j.target ?? '',
      message,
      raw,
    }
  } catch {
    return { time: '', level: '', target: '', message: raw, raw }
  }
}

/**
 * 查看某日的日志尾部。
 *
 * 直接调 `GET /api/diagnostics/logs/download?lines=N&date=...`（裸文本响应，
 * 见 `http.ts` 的 BARE_RESPONSE_PATHS），因此不会走 ApiResponse 信封解包。
 *
 * 失败语义与其余 api 模块一致：**原样 reject**，由页面 catch 后提示原因。
 * 早前在这里吞掉异常并返回 `{ error }`，调用方的 `catch` 因此失效，
 * 一旦页面漏读 `error` 字段，空表就会被当成「当天确实没有日志」。
 */
export async function viewArchive(date: string, lines = 200): Promise<ArchiveView> {
  const text = await request<string>({
    url: `/api/diagnostics/logs/download?lines=${lines}&date=${encodeURIComponent(date)}`,
    method: 'GET',
  })
  const body = typeof text === 'string' ? text : ''
  const parsed = body
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean)
    .map(parseLine)
  return { date, lines: parsed }
}

/**
 * 下载归档日志文件（浏览器本地保存）。
 *
 * 走 axios 而不是直接 `window.open`：下载接口需要 `Authorization` 头，
 * 直接开新窗口会丢 token 并拿到 401。
 */
export async function downloadArchive(date?: string): Promise<void> {
  const query = date ? `?lines=20000&date=${encodeURIComponent(date)}` : '?lines=20000'
  const text = await request<string>({ url: `/api/diagnostics/logs/download${query}`, method: 'GET' })
  const blob = new Blob([typeof text === 'string' ? text : ''], { type: 'text/plain;charset=utf-8' })
  triggerDownload(blob, date ? `gateway-${date}.log` : 'gateway-current.log')
}
