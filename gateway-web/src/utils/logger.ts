/**
 * 统一日志出口：分级输出，生产默认静默 debug/info，warn/error 保留（现场排障时有用）。
 *
 * 设计：
 *   - 分级：debug / info / warn / error。开发期全量输出；生产期 debug/info 静默，warn/error 继续写控制台。
 *   - scope：每条日志必须带 scope（文件/模块名），便于 grep 过滤。scope 规则 `module:detail`（如 `ws:pong` / `storage:decrypt`）。
 *   - 构建时可覆盖：VITE_LOG_VERBOSE=true 强制全开；VITE_LOG_MINIMUM=warn 强制只留 warn+error；生产构建默认 warn 级别。
 *   - 真正的错误上报：error 级同时写入 window.onerror 管线 / useErrorReporter（集中上报）。
 *   - ESLint 守护：`no-console: error` + 本文件白名单豁免，禁止业务代码直接 console.log。
 *
 * 禁止：在业务代码里写 `console.log` / `console.debug` / `console.info` / `console.error`（warn 仅本文件豁免）。
 * 用法：
 *   import { logDebug, logInfo, logWarn, logError } from '@/utils/logger'
 *   logInfo('ws:connect', '已连接网关 %s', url)
 *   logError('storage:decrypt', e, '密文解密失败，返回空')
 */

import { formatClock } from '@/utils/datetime'

export type LogLevel = 'debug' | 'info' | 'warn' | 'error'

const VERBOSE = import.meta.env.DEV || import.meta.env.VITE_LOG_VERBOSE === 'true'
const MINIMUM = (import.meta.env.VITE_LOG_MINIMUM || 'warn') as LogLevel

const ORDER: LogLevel[] = ['debug', 'info', 'warn', 'error']
const initialCutoff = VERBOSE ? (import.meta.env.DEV ? 'debug' : MINIMUM) : MINIMUM
let cutoffIdx = ORDER.indexOf(initialCutoff)

function shouldLog(level: LogLevel): boolean {
  return ORDER.indexOf(level) >= cutoffIdx
}

/** 格式化：把 console 的 %s/%d/%o 占位符展开，保留结构化对象输出 */
function format(scope: string, level: LogLevel, args: unknown[]): unknown[] {
  const prefix = `[${formatClock()}] [${level.toUpperCase()}] [${scope}]`
  // 字符串首参保留，便于 console 自带的格式化语义
  if (typeof args[0] === 'string') return [prefix + ' ' + args[0], ...args.slice(1)]
  return [prefix, ...args]
}

const raw = {
  debug: (...a: unknown[]) => console.debug(...a),
  info: (...a: unknown[]) => console.info(...a),
  warn: (...a: unknown[]) => console.warn(...a),
  error: (...a: unknown[]) => console.error(...a),
}

export function logDebug(scope: string, ...args: unknown[]): void {
  if (!shouldLog('debug')) return
  raw.debug(...format(scope, 'debug', args))
}

export function logInfo(scope: string, ...args: unknown[]): void {
  if (!shouldLog('info')) return
  raw.info(...format(scope, 'info', args))
}

export function logWarn(scope: string, ...args: unknown[]): void {
  if (!shouldLog('warn')) return
  raw.warn(...format(scope, 'warn', args))
}

/**
 * 错误级日志：除写控制台外，同时尝试走 window.onerror 管线（若在浏览器 + 非 jsdom 环境）。
 * 生产期会有上报管线挂到 window.onerror（见 useErrorReporter），这里配合走上报。
 * 测试期（jsdom）跳过 dispatchEvent，避免让 console 多出"未处理错误"噪音。
 */
export function logError(scope: string, err?: unknown, ...extra: unknown[]): void {
  if (!shouldLog('error')) return
  const formatted = format(scope, 'error', [err, ...extra])
  raw.error(...formatted)
  const isBrowser = typeof window !== 'undefined'
  const isJsdom = isBrowser && (window as unknown as { navigator?: { userAgent?: string } }).navigator?.userAgent?.includes('jsdom')
  const canDispatch = isBrowser && !isJsdom && typeof window.dispatchEvent === 'function'
  if (canDispatch && err instanceof Error) {
    const evt = new ErrorEvent('error', { message: `[${scope}] ${err.message}`, error: err })
    window.dispatchEvent(evt)
  }
}

/**
 * 测试工具：临时强制某个 scope 在某个 level 以上。
 * 默认 null = 按构建配置。仅 vitest 用；正式代码不要调用。
 */
export function __setLogCutoffForTests(level: LogLevel | null): void {
  if (level === null) {
    // 还原：与模块初始化保持一致
    cutoffIdx = ORDER.indexOf(VERBOSE ? 'debug' : MINIMUM)
  } else {
    cutoffIdx = ORDER.indexOf(level)
  }
}
