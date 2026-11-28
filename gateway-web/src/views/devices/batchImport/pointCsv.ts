/**
 * 批量导入 · 点位分支的 CSV 解析、默认值补齐与字段级合并（纯函数，不触碰组件状态）。
 *
 * 复用 @/utils/csvParse 的 RFC 4180 解析器（与 @/utils/batchImportCsv 同一底层实现），
 * 但 batchImportCsv 的 parseCSV 按固定 7 列（中文表头、设备专属字段）解析，覆盖不了点位表头，
 * 因此这里只保留「按列名定位 + 点位必填校验 + 默认属性补齐」这段点位专属映射，
 * 不搬用原页面的整套重复流程代码。
 *
 * 两种写入语义（导入页可选）：
 * - 覆盖（replace）：整点替换，CSV 没提到的字段一律重置为默认值（历史行为）；
 * - 合并（merge）：以现场现有点位为基底，只覆盖 CSV 里**出现过**的列，
 *   没出现的列保留原值，出现的列留空表示清空该字段。
 *
 * 典型应用场景：批量导入页点位分支选择 CSV 方式后的「校验」动作。
 */
import { parseCsvRows } from '@/utils/csv/csvParse'
import {
  POINT_FIELDS_FULL,
  POINT_FIELD_BY_COLUMN,
  parsePointCell,
} from '@/utils/device/pointFields'
import type { PointConfig } from '@/api/types'
import { defaultPointConfig as defaultPointValues } from '@/utils/config/pointDefaults'
import type { PointParseResult, PointRow } from './types'

/** CSV 解析结果：成功时带回校验结果与待提交数据，失败时带回提示文案 */
export type PointCsvParseOutcome =
  | { ok: true; result: PointParseResult; payload: PointConfig[]; presentColumns: string[] }
  | { ok: false; message: string }

/** 写入语义 */
export type PointWriteMode = 'merge' | 'replace'

/**
 * 新增点位的默认基底（字段与后端 serde default 对齐）。
 *
 * 转发到 @/utils/pointDefaults —— 该文件是「点位默认值」的唯一出处，
 * 批量导入 / 模板展开 / 批量生成 / 向导添加四条路径共用同一份语义。
 */
export function defaultPointConfig(): PointConfig {
  return defaultPointValues()
}

/** 解析点位 CSV：按列名定位字段并逐行校验 */
export function parsePointCsvRows(rawText: string): PointCsvParseOutcome {
  if (!rawText) return { ok: false, message: '请先上传文件' }
  const parsed = parseCsvRows(rawText)
  if (parsed.length < 2) return { ok: false, message: '文件内容不足，请确保包含表头和至少一行数据' }

  // 表头按列名定位：列顺序调整也能正确映射
  const headers = parsed[0]!.fields.map((h) => h.toLowerCase().trim())
  const col = (name: string) => headers.indexOf(name)
  const cell = (fields: string[], i: number) => (i >= 0 ? fields[i] ?? '' : '')
  /** 表头里真正出现过的可映射列（合并模式据此判断「哪些字段要覆盖」） */
  const presentColumns = headers.filter((h) => POINT_FIELD_BY_COLUMN.has(h))
  /** 变比单元格：留空返回 undefined；非法（非数字 / <= 0）记一条错误 */
  const parseRatio = (raw: string, name: string, errors: string[]): number | undefined => {
    if (!raw) return undefined
    const v = Number(raw)
    if (!Number.isFinite(v) || v <= 0) {
      errors.push(`${name} 必须大于 0（无需变比请留空）`)
      return undefined
    }
    return v
  }
  const idx = {
    id: col('id'), name: col('name'), deviceId: col('device_id'), bus: col('bus'),
    protocol: col('protocol'), address: col('address'), slaveId: col('slave_id'), addressNum: col('address_num'),
    ctRatio: col('ct_ratio'), ptRatio: col('pt_ratio'),
  }

  const rows: PointRow[] = []
  let valid = 0
  let invalid = 0

  for (const { line, fields } of parsed.slice(1)) {
    const id = cell(fields, idx.id)
    const bus = cell(fields, idx.bus)
    const protocol = cell(fields, idx.protocol)
    const address = cell(fields, idx.address)
    const slaveIdStr = cell(fields, idx.slaveId)

    const errors: string[] = []
    if (!id) errors.push('缺少 id')
    if (!bus) errors.push('缺少 bus')
    if (!protocol) errors.push('缺少 protocol')
    if (!address) errors.push('缺少 address')
    // modbus 协议靠从站地址寻址
    if (protocol.toLowerCase() === 'modbus' && !slaveIdStr) errors.push('modbus 协议需要 slave_id')
    // CT/PT 变比：留空=不启用；填了必须 > 0（0 会把一次侧值乘成 0）
    const ctRatio = parseRatio(cell(fields, idx.ctRatio), 'ct_ratio', errors)
    const ptRatio = parseRatio(cell(fields, idx.ptRatio), 'pt_ratio', errors)
    // 其余数字列：非数字直接判无效，避免把坏值写进配置
    for (const header of presentColumns) {
      const spec = POINT_FIELD_BY_COLUMN.get(header)!
      if (spec.kind !== 'number') continue
      if (header === 'ct_ratio' || header === 'pt_ratio') continue // 已单独校验
      const raw = (fields[headers.indexOf(header)] ?? '').trim()
      if (!raw) continue
      const parsedCell = parsePointCell(spec, raw)
      if (!parsedCell.ok) errors.push(parsedCell.error)
    }

    const row: PointRow = {
      line,
      id,
      name: cell(fields, idx.name),
      device_id: cell(fields, idx.deviceId),
      bus,
      protocol,
      address,
      ct_ratio: ctRatio,
      pt_ratio: ptRatio,
      valid: errors.length === 0,
    }
    if (slaveIdStr) {
      const sid = Number(slaveIdStr)
      row.slave_id = Number.isNaN(sid) ? undefined : sid
    }
    const addressNumStr = cell(fields, idx.addressNum)
    if (addressNumStr) {
      const an = Number(addressNumStr)
      row.address_num = Number.isNaN(an) ? undefined : an
    }
    // 原始单元格留档：合并模式要按「CSV 里出现过这一列」来覆盖，空单元格表示清空
    row.cells = Object.fromEntries(headers.map((h, i) => [h, fields[i] ?? '']))

    if (errors.length > 0) {
      row.error = errors.join('; ')
      invalid++
    } else {
      valid++
    }
    rows.push(row)
  }

  return {
    ok: true,
    result: { total: rows.length, valid, invalid, rows },
    payload: buildPointPayload(rows.filter((r) => r.valid)),
    presentColumns,
  }
}

/** CSV 行的默认值补齐（JSON 粘贴路径不走这里，保留用户原值） */
export function buildPointPayload(validRows: PointRow[]): PointConfig[] {
  return validRows.map((r) => ({
    ...defaultPointConfig(),
    id: r.id!,
    name: r.name ?? null,
    device_id: r.device_id ?? null,
    bus: r.bus!,
    protocol: r.protocol!,
    address: r.address!,
    slave_id: r.slave_id ?? null,
    address_num: r.address_num ?? null,
    ct_ratio: r.ct_ratio ?? null,
    pt_ratio: r.pt_ratio ?? null,
  }))
}

/**
 * 合并模式下的待提交数据：以现场现有点位为基底，只覆盖 CSV 里出现过的列。
 *
 * - 现有点位不存在（新增）→ 以默认基底起步，等价于覆盖模式；
 * - CSV 没出现过的列 → 保留原值（这是合并模式的意义：导出精简 10 列再导回，
 *   scale / unit / 公式 / 告警阈值等都不会被冲掉）；
 * - CSV 出现但留空的列 → 按字段表的语义清空或保留（scale/offset 这类不可清空字段
 *   留空表示保留原值，不会把点位值乘成 0）。
 *
 * @param existing 现场现有点位（按 id 索引）
 * @param presentColumns CSV 表头里真正出现过的可映射列
 */
export function buildMergedPayload(
  validRows: PointRow[],
  existing: Map<string, PointConfig>,
  presentColumns: Set<string>,
): PointConfig[] {
  return validRows.map((r) => {
    const base = existing.get(r.id ?? '') ?? defaultPointConfig()
    const merged = { ...base }
    const target = merged as unknown as Record<string, unknown>
    for (const spec of POINT_FIELDS_FULL) {
      if (!presentColumns.has(spec.column)) continue
      const parsed = parsePointCell(spec, r.cells?.[spec.column] ?? '')
      if (!parsed.ok) continue // 校验阶段已拦，这里兜底跳过
      if (parsed.value === undefined) continue // 不可清空字段留空 = 保留原值
      target[spec.key] = parsed.value
    }
    return merged
  })
}
