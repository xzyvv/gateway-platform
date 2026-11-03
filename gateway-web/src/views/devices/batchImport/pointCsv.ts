/**
 * 批量导入 · 点位分支的 CSV 解析与默认值补齐（纯函数，不触碰组件状态）。
 *
 * 复用 @/utils/csvParse 的 RFC 4180 解析器（与 @/utils/batchImportCsv 同一底层实现），
 * 但 batchImportCsv 的 parseCSV 按固定 7 列（中文表头、设备专属字段）解析，覆盖不了点位表头，
 * 因此这里只保留「按列名定位 + 点位必填校验 + 默认属性补齐」这段点位专属映射，
 * 不搬用原页面的整套重复流程代码。
 *
 * 典型应用场景：批量导入页点位分支选择 CSV 方式后的「校验」动作。
 */
import { parseCsvRows } from '@/utils/csvParse'
import type { PointConfig } from '@/api/types'
import type { PointParseResult, PointRow } from './types'

/** CSV 解析结果：成功时带回校验结果与待提交数据，失败时带回提示文案 */
export type PointCsvParseOutcome =
  | { ok: true; result: PointParseResult; payload: PointConfig[] }
  | { ok: false; message: string }

/** 解析点位 CSV：按列名定位字段并逐行校验 */
export function parsePointCsvRows(rawText: string): PointCsvParseOutcome {
  if (!rawText) return { ok: false, message: '请先上传文件' }
  const parsed = parseCsvRows(rawText)
  if (parsed.length < 2) return { ok: false, message: '文件内容不足，请确保包含表头和至少一行数据' }

  // 表头按列名定位：列顺序调整也能正确映射
  const headers = parsed[0]!.fields.map((h) => h.toLowerCase().trim())
  const col = (name: string) => headers.indexOf(name)
  const cell = (fields: string[], i: number) => (i >= 0 ? fields[i] ?? '' : '')
  const idx = {
    id: col('id'), name: col('name'), deviceId: col('device_id'), bus: col('bus'),
    protocol: col('protocol'), address: col('address'), slaveId: col('slave_id'), addressNum: col('address_num'),
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

    const row: PointRow = {
      line,
      id,
      name: cell(fields, idx.name),
      device_id: cell(fields, idx.deviceId),
      bus,
      protocol,
      address,
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
  }
}

/** CSV 行的默认值补齐（JSON 粘贴路径不走这里，保留用户原值） */
export function buildPointPayload(validRows: PointRow[]): PointConfig[] {
  return validRows.map((r) => ({
    id: r.id!,
    name: r.name ?? null,
    device_id: r.device_id ?? null,
    bus: r.bus!,
    protocol: r.protocol!,
    address: r.address!,
    slave_id: r.slave_id ?? null,
    address_num: r.address_num ?? null,
    data_id: null,
    function_code: null,
    data_type: null,
    byte_order: null,
    register_count: null,
    interval: 1,
    scale: 1,
    offset: 0,
    formula: null,
    ct_ratio: null,
    pt_ratio: null,
    deadband: 0,
    on_change: false,
    unit: null,
    alarm_high: null,
    alarm_low: null,
    aggregation: null,
    modbus_register: null,
    // S11: 批量导入的点位默认不在写入白名单内（避免误写现场寄存器）
    writable: false,
  }))
}
