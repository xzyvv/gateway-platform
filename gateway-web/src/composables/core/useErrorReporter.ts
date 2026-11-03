import { ref } from 'vue'
import { logError } from '@/utils/logger'

/**
 * 错误上报 composable（页面级边界配套）
 *
 * 设计目标：
 *   - 默认实现：console.error（与现有 silent 行为对齐），便于调试且不引入外部依赖
 *   - 接口与具体后端/埋点 SDK 解耦，未来接入 Sentry/自建 logger 时只替换 reporter
 *   - 测试可注入 mock reporter，验证边界触发是否被正确上报
 *
 * 边界组件调用 reportPageError() 时统一传：
 *   - boundary：边界所属位置（默认 'page'，可传 'route'、'hub' 等）
 *   - name：边界标识（路由路径 / Hub 名 / 组件名）
 *   - error：捕获的错误对象
 *   - info：附加上下文（Vue 组件栈 / 路由信息）
 */

export interface PageErrorReport {
  /** 边界类型：页面级 / 路由级 / 自定义 */
  boundary: 'page' | 'route' | string
  /** 边界名称（路由路径或组件名） */
  name: string
  /** 错误对象 */
  error: unknown
  /** 时间戳（ms） */
  timestamp: number
  /** 路由路径（如有） */
  route?: string
  /** 附加上下文（组件栈、环境等） */
  info?: Record<string, unknown>
}

export type ErrorReporter = (report: PageErrorReport) => void

/**
 * 默认 reporter：开发期写 console.error，生产期可在此处接入真实埋点
 * （例如 sendBeacon 到 /api/telemetry）。保持函数纯粹，便于测试覆盖。
 */
function defaultReporter(report: PageErrorReport): void {
  logError('err-rpt', report.error, `[PageErrorBoundary:${report.boundary}] ${report.name}`, report.info ?? '')
}

/** 当前激活的 reporter（默认 = defaultReporter） */
let activeReporter: ErrorReporter = defaultReporter

/** 设置 reporter（测试 / 真实埋点接入入口） */
export function setErrorReporter(reporter: ErrorReporter): void {
  activeReporter = reporter
}

/** 重置为默认 reporter（测试 teardown 用） */
export function resetErrorReporter(): void {
  activeReporter = defaultReporter
}

/**
 * 报告一次页面错误。集中组装 report 对象，避免每个边界各传一堆参数。
 * 直接调用 activeReporter，便于将来加缓冲/聚合时只改一处。
 */
export function reportPageError(input: {
  boundary: PageErrorReport['boundary']
  name: string
  error: unknown
  route?: string
  info?: Record<string, unknown>
}): void {
  activeReporter({
    ...input,
    timestamp: Date.now(),
  })
}

/**
 * 错误计数器（按 name 聚合）：用于错误徽章 / 上限防刷。
 * 返回的 counter 是响应式的，可直接在模板中绑定。
 */
// 有意的全局单例：错误计数需跨页面/边界共享（见 eslint.config.js #17 约定）
// eslint-disable-next-line no-restricted-syntax
const countByName = ref<Record<string, number>>({})

/** 增加某边界的错误计数（已达 99 时不再涨，避免破百后样式异常） */
export function bumpErrorCount(name: string, max = 99): number {
  const cur = countByName.value[name] ?? 0
  const next = cur >= max ? max : cur + 1
  countByName.value = { ...countByName.value, [name]: next }
  return next
}

/** 获取某边界的当前计数 */
export function getErrorCount(name: string): number {
  return countByName.value[name] ?? 0
}

/** 重置指定边界计数（边界恢复后调用） */
export function clearErrorCount(name: string): void {
  if (name in countByName.value) {
    const next = { ...countByName.value }
    delete next[name]
    countByName.value = next
  }
}

/** 响应式计数表（用于全局错误指示器） */
export function useErrorCounts() {
  return countByName
}