/**
 * HTTP 传输层（barrel）
 *
 * axios 实例 + 请求/响应拦截器 + request 包装
 *
 * - 请求拦截器：自动附带 Bearer Token + 记录起始时间戳 + 注册 AbortController
 * - 响应拦截器：解开 ApiResponse{code,message,data} 包装、统一错误处理、
 *   401 单飞登出、429 限速提示、写入请求日志
 * - 全局 in-flight 控制：切换网关 / 登出时 abortAllInflight 把"切网之前的请求"
 *   统一取消，避免请求打到错误的网关或被静默丢弃（注册中心实现见 ../inflight）
 *
 * 模块拆分（每个模块职责单一）：
 * - ./trace：响应元信息（trace ID / 离线陈旧标记）
 * - ./sanitize：请求日志脱敏
 * - ./loggingOut：401 单飞登出门（引用计数）
 * - ./envelope：统一信封（ApiEnvelope）与「裸响应路径」白名单
 * - ./instance：axios 实例 + 请求拦截器 + TimedConfig
 * - ./responseInterceptor：响应拦截器（解包 + 错误处理 + 日志）
 * - ./request：核心请求函数（request / requestWithMeta）
 * - ./network：网络状态监听（断网提示）
 *
 * 本文件作为 barrel 重新导出，保持 '@/api/http' 与 api barrel 的既有 import 路径不变，
 * 全站调用方无需改动。
 */
import './network'
import './responseInterceptor'

export { instance } from './instance'
export { request, requestWithMeta } from './request'
export { readTraceId } from './trace'
export type { ResponseMeta } from './trace'
export { sanitizeForLog, sanitizeUrl, loggableHeaders } from './sanitize'
export {
  tryAcquireLoggingOut,
  releaseLoggingOut,
  markLoggingOut,
  resetLoggingOut,
  getLoggingOutCount,
} from './loggingOut'
// 在飞请求注册中心独立成模块（inflight.ts），切断与 gatewayConfig.ts 的循环依赖。
// 此处再导出，保持 '@/api/http' 与 api barrel 的既有 import 路径不变。
export {
  onInflightCountChange,
  getInflightCount,
  abortAllInflight,
  triggerGatewaySwitchRefresh,
} from '../inflight'

export { instance as default } from './instance'
