/**
 * RFC 4180 CSV 解析（纯前端，无第三方依赖）
 *
 * 为什么需要：多处 CSV 导入此前用 `line.split(',')` 逐行切列，
 * 设备名/点位名里含逗号（如「电表,1号」）时字段被切成两列——
 * 后面的字段整体左移一位，**数据被静默损坏**：
 *   - 切列后总线名落到协议列 → 报「协议须为 modbus-tcp」，归因完全误导；
 *   - 逗号出现在最后一列时 → 该行**通过校验**，多余字段被无声丢弃。
 *
 * 本解析器按 RFC 4180 处理：
 *   - 双引号包裹的字段可含逗号 / 换行 / 双引号（"" 转义为一个双引号）；
 *   - 未包裹的字段按字面处理；
 *   - 行号按**物理行**计数并回传（引号内的换行不另起一行）。
 *
 * 与 exportCsv.ts 的 sanitizeCsvCell / csvEscapeFieldOptionalQuote 是配套的
 * （导出时按需加引号，解析时还原），两边口径一致。
 */

export interface ParsedCsvRow {
  /** 物理行号（1-based） */
  line: number
  /** 该行的字段（已去首尾空白、已还原转义） */
  fields: string[]
}

/**
 * 解析 CSV 文本为行数组。
 *
 * @param text        CSV 文本内容
 * @param opts.skipEmpty 是否丢弃全空行（默认 true）
 * @returns 行数组；每行带 1-based 物理行号
 */
export function parseCsvRows(text: string, opts: { skipEmpty?: boolean } = {}): ParsedCsvRow[] {
  const skipEmpty = opts.skipEmpty ?? true
  const rows: ParsedCsvRow[] = []
  if (!text) return rows

  let fields: string[] = []
  let current = ''
  let inQuotes = false
  /** 当前字段是否被引号包裹：被包裹时内容原样保留（不 trim） */
  let quoted = false
  let line = 1
  let rowStartLine = 1
  let sawAnyChar = false

  const endField = () => {
    // 引号包裹的字段内容原样保留（RFC 4180：引号内的空格是数据的一部分）
    fields.push(quoted ? current : current.trim())
    current = ''
    quoted = false
  }
  const endRow = () => {
    endField()
    const isEmpty = fields.length === 1 && fields[0] === ''
    if (!skipEmpty || !isEmpty || sawAnyChar) {
      rows.push({ line: rowStartLine, fields })
    }
    fields = []
    sawAnyChar = false
  }

  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!
    if (inQuotes) {
      if (ch === '"') {
        // "" → 字面双引号（RFC 4180 转义）
        if (text[i + 1] === '"') {
          current += '"'
          i++
        } else {
          inQuotes = false
        }
      } else {
        // 引号内的换行属于字段内容，不是新行
        if (ch === '\n') line++
        current += ch
      }
      continue
    }
    if (ch === '"' && current.trim() === '') {
      // 字段起始处的引号 → 进入引号模式
      inQuotes = true
      quoted = true
      current = ''
      sawAnyChar = true
      continue
    }
    if (ch === ',') {
      endField()
      sawAnyChar = true
      continue
    }
    if (ch === '\r') {
      // 吞掉 CRLF 的 \r，交给 \n 处理行结束
      continue
    }
    if (ch === '\n') {
      endRow()
      line++
      rowStartLine = line
      continue
    }
    current += ch
    sawAnyChar = true
  }
  // 文件末尾没有换行时补上最后一行
  if (current !== '' || fields.length > 0) {
    endRow()
  }
  return rows
}

/**
 * 解析 CSV 为「字段数组的数组」（不含行号）。
 *
 * 便捷入口：多数调用方只关心字段内容。
 */
export function parseCsv(text: string, opts: { skipEmpty?: boolean } = {}): string[][] {
  return parseCsvRows(text, opts).map((r) => r.fields)
}
