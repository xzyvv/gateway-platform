/**
 * 日期 / 时间格式化唯一出口
 *
 * 为什么统一：
 * 过去各页面直接调用 `new Date(x).toLocaleString(...)`，参数写法不一致——有的带
 * `{ hour12: false }`（24 小时制），有的不带（中文环境默认 12 小时制，带「上午/下午」）。
 * 结果同一界面上会同时出现 "2026/10/24 12:31:14" 与 "2026/10/24 下午12:31:14"，
 * 用户无法横向对比时间，也给日志排查带来歧义。收敛到本模块后，所有展示统一为
 * 24 小时制，格式调整只需改这一处。
 *
 * 约定：
 * - 无效输入（null / undefined / 空串 / NaN / Invalid Date）一律返回空字符串 ''，
 *   避免 "Invalid Date" 直接泄漏到界面或导出文件里。
 * - 不做时区转换，按浏览器本地时区展示（与后端时间戳含义一致）。
 * - 不引入 dayjs 等第三方库，保持零依赖。
 */

type DateInput = number | string | Date | null | undefined

/** 把外部输入安全地转成 Date；无效输入返回 null */
function toDate(input: DateInput): Date | null {
  if (input == null || input === '') return null
  const d = input instanceof Date ? input : new Date(input)
  return Number.isNaN(d.getTime()) ? null : d
}

/**
 * 完整日期时间（24 小时制），形如 2026/10/24 12:31:14。
 * 适用场景：表格时间列、详情页时间字段、导出文件、日志记录等需要完整时间的展示。
 */
export function formatDateTime(input: DateInput): string {
  const d = toDate(input)
  return d ? d.toLocaleString('zh-CN', { hour12: false }) : ''
}

/**
 * 仅日期，形如 2026/10/24。
 * 适用场景：到期日、创建日期等只关心年月日的字段，避免多余时间干扰阅读。
 */
export function formatDate(input: DateInput): string {
  const d = toDate(input)
  return d ? d.toLocaleDateString('zh-CN') : ''
}

/**
 * 仅时间（24 小时制），形如 12:31:14。
 * 适用场景：报文日志、实时数据流等时间跨度在同一自然日内、只需时分秒的展示。
 */
export function formatTimeOfDay(input: DateInput): string {
  const d = toDate(input)
  return d ? d.toLocaleTimeString('zh-CN', { hour12: false }) : ''
}

/**
 * 当前时刻的时间字符串（24 小时制），供顶栏时钟、实时状态条等组件使用。
 * 语义上等价于 formatTimeOfDay(Date.now())，单独封装以便调用处语义清晰。
 */
export function formatClock(): string {
  return formatTimeOfDay(Date.now())
}
