/**
 * 容量与时长的人类可读格式化（唯一定义处）
 *
 * 为什么要收成一个模块：这两类格式化过去在 6 个文件里各写一遍，口径已经漂了——
 * - 字节：`2.00 MB`（配置版本）/ `2.0 MB`（网关诊断）/ `1536 KB`（日志归档，KB 取整）
 *   —— 同一个大小在不同页显示不同，现场截图对不上；
 * - 时长：`1.3s`（报文回放）/ `1.30 秒`（健康评分）并存。
 *
 * 约定（改动前请先确认没有按旧口径写死的用例）：
 * - 字节：B 取整；KB 及以上统一 **1 位小数**，单位按 1024 进位（与后端 `queue` 容量
 *   口径一致，不用 SI 的 1000）；
 * - 非法值（NaN / Infinity / 负数）一律降级成 `0 B` / `-`，不把 "NaN KB" 抛到界面上；
 * - 时长分两种语义，别混用：
 *   - [`formatDuration`] 是「多久」的长跨度（排队积压 2 天 3 小时），带天/小时/分；
 *   - [`formatElapsed`] 是「耗时」的短跨度（一次读取 850 ms / 1.25 秒）。
 *   - [`formatRelative`] 是「距今多久」的相对时间（N 秒前 / N 分钟前 / N 小时前）。
 */

/** 字节单位（1024 进位） */
const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const

/**
 * 字节数 → 可读文本：`0 B` / `512 B` / `1.5 KB` / `2.0 MB` / `1.0 GB`。
 *
 * 超过 TB 仍按 TB 显示（工业网关上不会出现，避免为不可能的分支加单位）。
 */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  let value = bytes
  let unit = 0
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024
    unit += 1
  }
  if (unit === 0) {
    // B 不带小数（0.5 B 没有意义）
    return `${Math.round(value)} B`
  }
  // 四舍五入后可能顶到 1024（如 1048575 B → 1024.0 KB），必须进一位，
  // 否则会出现「1024.0 KB」这种一眼看着像溢出的写法
  let rounded = Number(value.toFixed(1))
  if (rounded >= 1024 && unit < BYTE_UNITS.length - 1) {
    unit += 1
    rounded = value / 1024
  }
  return `${rounded.toFixed(1)} ${BYTE_UNITS[unit]}`
}

/**
 * 毫秒 → 长跨度时长：`2 天 3 小时` / `3 小时 5 分 8 秒` / `5 分 8 秒` / `42 秒`。
 *
 * 用于「排队多久了」这类按天计的展示：精确到分即可，不再往下拆。
 * `null` / 非有限值返回 `-`（后端未给出时间戳时不能显示成 0 秒）。
 */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return '-'
  const total = Math.floor(ms / 1000)
  const days = Math.floor(total / 86400)
  const hours = Math.floor((total % 86400) / 3600)
  const minutes = Math.floor((total % 3600) / 60)
  const seconds = total % 60
  if (days > 0) return `${days} 天 ${hours} 小时 ${minutes} 分`
  if (hours > 0) return `${hours} 小时 ${minutes} 分 ${seconds} 秒`
  // 不足 1 分钟不写成「0 分 42 秒」：那个 0 分只会让人多读一遍
  if (minutes === 0) return `${seconds} 秒`
  return `${minutes} 分 ${seconds} 秒`
}

/**
 * 毫秒 → 短耗时：`850 ms` / `1.25 秒` / `5 秒`。
 *
 * 不足 1 秒用毫秒（现场排查串口超时看的是 ms）；1 秒以上保留至多 2 位小数，
 * 并去掉无意义的尾零（`5.00 秒` 读起来像精度承诺）。
 */
export function formatElapsed(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms) || ms < 0) return '-'
  // 先四舍五入再判单位：999.6 ms 该显示「1 秒」，不是「1000 ms」
  const rounded = Math.round(ms)
  if (rounded < 1000) return `${rounded} ms`
  return `${Number((rounded / 1000).toFixed(2))} 秒`
}

/**
 * 相对时间：距今多少秒 → `N 秒前` / `N 分钟前` / `N 小时前`。
 *
 * 为什么要收口：这段「三档阈值 + 换算」过去在 5 个文件里各写一遍，口径已经漂了——
 * 有的写「3秒前」（无空格），有的写「3 秒前」（有空格）；有的对 5 秒内返回「刚刚」，
 * 有的直接显示「0 秒前」。同一批数据在顶栏横幅和表格徽章上显示得不一样。
 *
 * 约定：
 * - 单位之间**带空格**（与本模块其它格式化函数一致）；
 * - 5 秒内返回「刚刚」——「0 秒前」看着像坏了，实际是数据刚到；
 * - 超过一天仍按小时计（「25 小时前」），不引入「N 天前」，避免同一条数据
 *   在不同页面跳档；
 * - 非法值（NaN / Infinity / 负数）返回 `-`，不把 "NaN 秒前" 抛到界面上。
 *
 * @param seconds 距今的秒数
 */
export function formatRelative(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return '-'
  const s = Math.floor(seconds)
  if (s < 5) return '刚刚'
  if (s < 60) return `${s} 秒前`
  if (s < 3600) return `${Math.floor(s / 60)} 分钟前`
  return `${Math.floor(s / 3600)} 小时前`
}
