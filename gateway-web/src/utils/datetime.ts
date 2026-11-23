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

/**
 * 时间戳归一化策略（后端不同接口的时间单位不统一，这是客观事实）：
 * - `auto`：按 1e12 阈值自动判断（< 1e12 当秒，否则当毫秒）——绝大多数接口；
 * - `sec`：无条件 ×1000（配置版本等明确以秒为单位的接口）；
 * - `ms`：已是毫秒，不换算（历史查询导出等接口已保证单位）。
 */
type TimestampUnit = 'auto' | 'sec' | 'ms'

interface FormatTimestampOptions {
  /** 输入单位，默认 `auto` */
  unit?: TimestampUnit
  /** 输出样式：`datetime`（默认，完整日期时间）/ `time`（仅时分秒） */
  style?: 'datetime' | 'time'
  /** 空值 / 无效值兜底文案，默认空串 '' */
  fallback?: string
}

/**
 * 时间戳统一格式化：归一化（秒 / 毫秒）+ 委托 formatDateTime / formatTimeOfDay + 空值兜底。
 *
 * 为什么收敛到这里：曾散落在 6 个文件各写一份「t > 1e12 ? t : t*1000」，
 * 阈值写法还不统一（`> 1e12` vs `< 1e12`，边界值语义有分歧）。后端单位口径
 * 一旦调整，要改 6 处且容易漏。现在字段含义用 `unit` 显式声明，换算只此一处。
 *
 * 约定：
 * - `0` / 负数 / NaN / 非有限数一律视为「无有效时间」，返回 `fallback`；
 * - 归一化后仍无效时同样返回 `fallback`（不让 "Invalid Date" 泄漏到界面）；
 * - 不做时区转换，与 formatDateTime / formatTimeOfDay 一致。
 */
export function formatTimestamp(
  input: number | null | undefined,
  opts: FormatTimestampOptions = {},
): string {
  const { unit = 'auto', style = 'datetime', fallback = '' } = opts
  if (input == null || !Number.isFinite(input) || input <= 0) return fallback
  const ms = unit === 'sec' ? input * 1000 : unit === 'ms' ? input : (input < 1e12 ? input * 1000 : input)
  if (!Number.isFinite(ms)) return fallback
  const formatted = style === 'time' ? formatTimeOfDay(ms) : formatDateTime(ms)
  return formatted || fallback
}

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

/**
 * 两位补零（`7 → "07"`）。
 *
 * 只用于**布局不属于上面几个标准格式**的地方（如「11-17 22:04」这类紧凑标题、
 * `YYYYMMDD-HHMM` 形式的文件名）。此前这类地方各自在局部定义了一份完全相同的
 * `const pad = (n) => String(n).padStart(2, '0')`（8 处），改一处口径要改 8 个文件。
 *
 * 需要「HH:mm:ss」「YYYY/MM/DD HH:mm:ss」时请直接用 `formatTimeOfDay` /
 * `formatDateTime`，不要用本函数自己拼。
 */
export function pad2(n: number): string {
  return String(n).padStart(2, '0')
}
