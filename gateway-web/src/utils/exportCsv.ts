import type { DataPoint } from '@/api/types'
import { formatDateTime } from '@/utils/datetime'

/**
 * CSV 导出统一出口
 *
 * - `toCsv` / `downloadCsv`：实时数据点（DataPoint[]）导出，RFC4180 选择性转义 + UTF-8 BOM
 * - `buildHistoryCsv`：历史查询结果的通用 CSV 构建（字段统一双引号包裹，**不含** BOM，
 *   由调用方按需前置 `\ufeff`）
 *
 * 三者的 blob 下载均复用同一 `triggerDownload`，避免各处重复编写
 * `URL.createObjectURL` + `<a download>` 样板。
 *
 * W-7 修复：CSV 公式注入防护。
 * 旧实现只做 RFC 4180 转义（双引号双写 + 逗号/换行/引号触发整体加引号），
 * 但**没**防御 CSV/Excel 公式注入。设备名/点位名/版本号/用户备注等可控字符串
 * 若以 `=cmd|'/c calc'!A0`、`@SUM(1+9)`、`+2+3`、`-1+1`、`\t=...` 开头，
 * 在 Excel/Google Sheets/LibreOffice Calc 打开时会被解析为公式执行：
 *
 *   | 单元格内容           | Excel 行为              |
 *   |----------------------|-------------------------|
 *   | =cmd|'/c calc'!A0   | 执行 calc（Windows）     |
 *   | @SUM(1+9)            | 公式求值                |
 *   | +1+1                 | 算术                    |
 *   | -2*3+SUM(A1)         | 公式                    |
 *   | \t=...               | 制表符 + 等号也触发     |
 *
 * 修复策略（OWASP 推荐）：
 * 1. 单元格值若以 `=` / `+` / `-` / `@` / `\t` / `\r` 之一开头 → **加单引号 `'` 前缀**，
 *    告诉电子表格"这是文本不是公式"（CSV 标准做法，跨 Excel/Sheets/LibreOffice 兼容）
 * 2. 单引号前缀后**仍**走 RFC 4180 转义（双引号双写 + 触发字符加引号）
 * 3. 所有导出路径（toCsv / buildHistoryCsv / quoteTimeField）统一走同一脱敏函数
 *    `sanitizeCsvCell`，避免某条路径漏防
 */

/**
 * CSV 危险前缀集合：单元格值以这些字符开头时，Excel/Sheets 会解析为公式
 *
 * Bug P2-B11 (fix): 旧列表缺 '\n'。LibreOffice Calc / 旧版 Numbers 在解析
 * CSV 时会把「以换行符起始的单元格」按公式处理（CVE-2014-3524 类行为）。
 * 补 '\n' 触发单引号前缀防御。
 */
const FORMULA_TRIGGERS: ReadonlyArray<string> = ['=', '+', '-', '@', '\t', '\r', '\n']

/**
 * 单引号前缀：把"以危险字符开头的值"标记为纯文本。
 * 单引号是 CSV 标准中"转义为文本"的语法——Excel/Sheets/LibreOffice 都识别。
 */
const FORMULA_ESCAPE_PREFIX = "'"

/**
 * 单元格脱敏：仅做公式注入防护，返回**不带外层双引号**的"裸"字符串。
 * 调用方根据需要决定是否再用 `csvEscapeField` 加 RFC 4180 双引号包裹。
 *
 * 步骤：
 * 1. 字符串化 null/undefined
 * 2. 危险前缀（= + - @ \t \r）加单引号前缀
 * 3. 内部双引号双写（即使最终不包外层引号也要做，
 *    因为 `toCsv` / `buildHistoryCsv` 都会加外层引号）
 */
export function sanitizeCsvCell(value: unknown): string {
  const str = value == null ? '' : String(value)
  if (str.length === 0) {
    return ''
  }
  let safe = str
  if (FORMULA_TRIGGERS.includes(str[0]!)) {
    safe = FORMULA_ESCAPE_PREFIX + str
  }
  // 内部双引号双写（RFC 4180），无论最终是否外层包裹
  return safe.replace(/"/g, '""')
}

/**
 * 完整 RFC 4180 转义：先 sanitize（公式注入防护 + 内部双引号双写），
 * 再决定是否外层双引号包裹。
 * 总是双引号包裹的版本：用于历史数据等"全部强引号"格式需求。
 */
export function csvEscapeField(value: unknown): string {
  return '"' + sanitizeCsvCell(value) + '"'
}

/**
 * 按需双引号包裹：含 , " \r \n 时才整体加双引号，
 * 否则裸值（已做公式防护 + 内部双引号双写）。
 *
 * 注意：sanitize 之后的 " 已被双写，但若原值含 " 仍触发整体加引号——
 * 实际等价于"原值含 , " \r \n"。这里在判断时直接用 `safe.includes('"')`
 * 是因为：含 " 的字段必须外层加引号才能让 CSV 解析器区分"内引号是字段内容"。
 */
export function csvEscapeFieldOptionalQuote(value: unknown): string {
  const safe = sanitizeCsvCell(value)
  if (/[,\r\n]/.test(safe) || safe.includes('"')) {
    return '"' + safe + '"'
  }
  return safe
}

/** 时间列始终用双引号包裹（本地化格式可能含逗号/空格），内部引号双写转义 */
function quoteTimeField(time: string): string {
  return csvEscapeField(time)
}

/**
 * 将实时数据点数组导出为 CSV 字符串（含 UTF-8 BOM 头，Excel 可正确显示中文）
 * 纯函数：不操作 DOM，便于单元测试
 */
export function toCsv(rows: DataPoint[]): string {
  const header = ['point_id', 'device_id', 'value', 'raw_value', 'raw_type', 'quality', 'timestamp', 'time']
  const lines = [header.map(csvEscapeFieldOptionalQuote).join(',')]
  for (const p of rows) {
    const time = p.timestamp ? formatDateTime(p.timestamp) : ''
    const fields = [
      p.point_id,
      p.device_id,
      p.value,
      p.raw_value,
      p.raw_type,
      p.quality,
      p.timestamp,
    ]
    lines.push(
      fields.map(csvEscapeFieldOptionalQuote).join(',') + ',' + quoteTimeField(time),
    )
  }
  return '\uFEFF' + lines.join('\n')
}

/**
 * 通用表格 CSV 下载：调试工具导出（抓包 / 扫描结果 / 操作记录）统一入口。
 * 单元格走 sanitizeCsvCell（公式注入防护）+ 按需加引号，前置 BOM 供 Excel 识别中文。
 */
export function downloadTableCsv(header: string[], rows: unknown[][], filename: string): void {
  const lines = [header, ...rows].map((r) => r.map(csvEscapeFieldOptionalQuote).join(','))
  triggerDownload(new Blob(['\uFEFF' + lines.join('\n')], { type: 'text/csv;charset=utf-8' }), filename)
}

/**
 * 触发浏览器文件下载（创建临时 object URL 并点击 a 标签后回收）
 * 依赖 DOM，测试时可 mock
 */
export function triggerDownload(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  a.click()
  URL.revokeObjectURL(url)
}

/**
 * 下载一段「已拼好的」CSV 文本（自动前置 UTF-8 BOM）。
 *
 * 供 `buildHistoryCsv` 结果、诊断统计报告、批量导入模板等「先拼字符串再下载」
 * 的路径复用，避免各处手写 `new Blob(['\ufeff' + csv], ...)`：
 * 少写一次 BOM，Excel 打开中文就是乱码（W3a-5 前 `PointImportBranch.vue` 即如此）。
 */
export function downloadCsvText(csv: string, filename: string): void {
  triggerDownload(new Blob(['\uFEFF' + csv], { type: 'text/csv;charset=utf-8' }), filename)
}

/** 在浏览器中触发实时数据 CSV 下载 */
export function downloadCsv(rows: DataPoint[], filename?: string): void {
  const name = filename || `realtime-data-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.csv`
  triggerDownload(new Blob([toCsv(rows)], { type: 'text/csv;charset=utf-8' }), name)
}

/**
 * 构建历史数据 CSV 文本（含表头；字段统一双引号包裹，内部引号双写转义）
 *
 * W-7 修复：每格先走 sanitizeCsvCell（防御公式注入 + RFC 4180），
 * 再统一加双引号包裹。Excel 即使在双引号包裹的单元格中**仍会执行**
 * `=cmd|...` 形式公式——单引号前缀是关键防线。
 */
export function buildHistoryCsv(
  rows: (string | number)[][],
  header: string[] = ['时间', '点位', '值', '质量'],
): string {
  // W-7 修复：每格走 csvEscapeField（防御公式注入 + RFC 4180 + 全部加引号）。
  // 历史数据格式文档约定全字段双引号包裹，调用方按规范用 csvEscapeField。
  return [header, ...rows]
    .map((row) => row.map((c) => csvEscapeField(c)).join(','))
    .join('\n')
}
