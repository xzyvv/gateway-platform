/**
 * 统一响应信封（ApiEnvelope）与「裸响应路径」白名单
 *
 * 后端 router 层已把 /api/* 的 2xx JSON 统一包成 ApiResponse 信封
 * （gateway-api/src/web/middleware.rs::envelope），前端**只按信封解包**，
 * 不再靠「猜 body 里有没有 code/data/message」来决定是否解包——
 * 那种猜法会让某个 handler 漏改造后静默返回整个信封对象。
 */

/** ApiResponse 信封（后端 `gateway-api/src/web/types.rs::ApiResponse` 的线格式） */
export type ApiEnvelope = { code: number; message?: string; data: unknown }

/**
 * 豁免统一信封的响应路径 —— 必须与后端
 * `gateway-api/src/web/middleware.rs::is_envelope_exempt` 保持一致。
 *
 * - `/health/*`：K8s/负载均衡探针读裸 JSON（就绪探针直接读 `status` 字段）
 * - `/api/metrics`：Prometheus `text/plain`
 * - `/api/history/export`：CSV 下载（按 blob 消费）
 * - `/api/auth/login`：裸 `{ token }` 契约（见 api/auth.ts）
 * - `/api/diagnostics/logs/download`：日志文本下载
 */
export const BARE_RESPONSE_PATHS = [
  '/api/metrics',
  '/api/history/export',
  '/api/auth/login',
  '/api/diagnostics/logs/download',
]

export function isBareResponse(url: string | undefined): boolean {
  if (!url) return false
  const path = url.split('?')[0] ?? ''
  return path === '/health' || path.startsWith('/health/') || BARE_RESPONSE_PATHS.includes(path)
}
