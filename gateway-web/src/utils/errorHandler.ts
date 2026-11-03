/**
 * 从 catch 的 unknown 类型中提取错误消息。
 *
 * 统一替代各 composable / store / view 中重复出现的
 * `e instanceof Error ? e.message : String(e)` 及其变体模式。
 *
 * @param e        catch 块中的 unknown 值
 * @param fallback 当 e 不是 Error / string 时的兜底消息（如 null / undefined / number）
 */
export function extractErrorMessage(e: unknown, fallback?: string): string {
  if (e instanceof Error) return e.message
  if (typeof e === 'string') return e
  if (fallback !== undefined) return fallback
  return String(e)
}

/**
 * 写操作失败提示：「<动作>失败：<原因>」。
 *
 * 写操作的全局弹窗被拦截器静音（避免和页面提示叠两条），所以页面 catch 是用户看到
 * 失败原因的唯一出口。原因取自 `error.message`——拦截器已把 ApiError / ApiResponse
 * 的中文 message 回写到该字段，能区分配额超限、无写权限、id 冲突等可纠正错误；
 * 只写一句「请稍后重试」会让用户无从下手。
 *
 * 原因缺失时退回「<动作>失败，请稍后重试」，避免出现「失败：」这种半截文案。
 */
export function failureMessage(action: string, e: unknown): string {
  const reason = extractErrorMessage(e, '').trim()
  return reason ? `${action}失败：${reason}` : `${action}失败，请稍后重试`
}
