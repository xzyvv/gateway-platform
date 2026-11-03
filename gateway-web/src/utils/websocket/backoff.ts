// S12: 客户端 WebSocket 重连退避（指数 + jitter + 浏览器 online/offline 事件）。
/**
 * 计算重连延迟（指数退避 + jitter）。
 * - 公式：`min(maxDelay, baseDelay * 2^(attempt-1) * jitter)`
 * - jitter ∈ [0.7, 1.3]（±30%）——避免多 tab / 多实例同时重连打爆服务端
 * - attempt 从 1 开始；attempt<=0 时返回 baseDelay（防御性）
 *
 * 抽成可测纯函数，方便单测覆盖边界。
 */
export function computeReconnectDelay(
  attempt: number,
  baseDelay = 3000,
  maxDelay = 30000,
  rng: () => number = Math.random,
): number {
  const safeAttempt = Math.max(1, Math.floor(attempt));
  const exp = Math.min(maxDelay, baseDelay * 2 ** (safeAttempt - 1));
  const jitter = 0.7 + rng() * 0.6; // [0.7, 1.3)
  return Math.min(maxDelay, Math.round(exp * jitter));
}
