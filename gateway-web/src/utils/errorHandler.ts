/**
 * 错误文案的拼装与「是否已提示过」的标记
 *
 * 本模块只做纯函数/纯标记，不依赖 UI，便于单独测试。
 *
 * # 为什么要有 composeFailure
 *
 * 现场看到的失败提示必须同时回答两个问题：**哪个动作失败了**、**为什么失败**。
 * 只给原因（"总线不存在"）时用户不知道是自己刚点的「保存」还是后台轮询出了问题；
 * 只给动作（"保存失败"）时用户无从下手。历史上有两套写法并存：
 *
 *   - `failureMessage('保存点位', e)` → 「保存点位失败：<原因>」（动作不含「失败」）
 *   - `showToast(e, '保存点位失败')`   → 旧实现只回 `<原因>`，动作被丢掉
 *     （`extractErrorMessage` 的 fallback 只在抛出非 Error 时生效）
 *
 * 后者占多数（68 处），于是绝大多数失败提示都丢了动作语境。现在两种写法都收敛到
 * `composeFailure`：调用方照旧把完整动作短语（"保存点位失败"）传进来即可。
 */

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
 * 拼「动作 + 原因」：`<action>：<reason>`。
 *
 * 边界（都有用例钉住）：
 * - 原因为空 → 只回动作，避免出现「保存失败：」这种半截文案；
 * - 动作与原因相同 → 只回一份，避免「保存失败：保存失败」；
 * - 原因已以动作开头 → 只回原因，避免重复前缀；
 * - 动作为空 → 只回原因（调用方没给语境时不该凭空造一个「操作失败」）。
 */
export function composeFailure(action: string | null | undefined, reason: string): string {
  const a = (action ?? '').trim()
  const r = (reason ?? '').trim()
  if (!a) return r
  if (!r) return a
  if (r === a || r.startsWith(a)) return r
  return `${a}：${r}`
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
 *
 * @param action 动作短语，**不含**「失败」二字（如 "保存点位" / "删除设备"）
 */
export function failureMessage(action: string, e: unknown): string {
  const reason = extractErrorMessage(e, '').trim()
  if (!reason) return `${action}失败，请稍后重试`
  return composeFailure(`${action}失败`, reason)
}

/**
 * 「用户已经看到过这条错误的提示」的标记键。
 *
 * GET 请求失败时拦截器会弹全局通知（写操作是静音的，见 responseInterceptor 注释），
 * 而页面自己的 catch 往往还会再弹一条 Toast → 同一个错误两条提示。拦截器在弹过
 * 通知后给 Error 打上本标记，`useErrorDisplay.showToast` 据此跳过重复的那条。
 *
 * 用 Symbol 而非字符串属性：错误对象会被日志/遥测序列化，Symbol 键不会被枚举进去。
 */
const USER_NOTIFIED: unique symbol = Symbol('gateway.userNotified')

/** 标记「这条错误的提示已经由全局层发出」 */
export function markUserNotified(err: unknown): void {
  if (typeof err !== 'object' || err === null) return
  try {
    Object.defineProperty(err, USER_NOTIFIED, { value: true, enumerable: false, configurable: true })
  } catch {
    /* 被冻结的 Error 对象忽略即可：退化为旧行为（页面再弹一条） */
  }
}

/** 该错误是否已经由全局层提示过用户 */
export function isUserNotified(err: unknown): boolean {
  if (typeof err !== 'object' || err === null) return false
  return (err as Record<symbol, unknown>)[USER_NOTIFIED] === true
}
