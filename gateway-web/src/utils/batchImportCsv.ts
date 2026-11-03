/**
 * 设备批量导入：CSV 解析 / 逐行校验 / 模板下载（纯前端工具）
 *
 * 注意：parseCSV / validateImport 在页面中为同步调用，不能改成返回 Promise。
 * 提交下发由页面调用真实接口 `POST /api/devices` 完成（见 DeviceBatchImport.vue）。
 *
 * 模板列为何必填：后端 `create_device` 会校验模板存在性与「模板协议 = 设备协议」，
 * 缺模板或协议不符都会被直接拒绝（4001）。本地先校验能省掉一轮往返，并把
 * 可用模板列在报错里，避免用户对着「template not found」反复试错。
 *
 * 修复（P1-14）：原实现用 `split(',')` 逐行切列，设备名含逗号时字段整体左移，
 * 数据被**静默损坏**（切列后总线名落到协议列，报错完全误导；逗号在最后一列时
 * 整行通过校验、多余字段被无声丢弃）。现改用 RFC 4180 解析器
 * （见 @/utils/csvParse），并对列数不符的行显式报错。
 */
import { parseCsvRows } from './csvParse'
import { downloadCsvText } from './exportCsv'

/** 网关模板库里的模板（用于本地校验模板存在性与协议一致性） */
export interface TemplateInfo {
  name: string
  protocol?: string
}

export interface ImportRow {
  /** 源文件行号（1-based，表头为第 1 行） */
  line: number
  name: string
  id: string
  bus: string
  protocol: string
  slaveId: number | string
  /** 设备模板名（必填，须与网关模板库中的模板一致） */
  template: string
  /** 设备地址：Modbus 留空时取从站ID，其它协议必填 */
  address: string
  valid: boolean
  error?: string
}

export interface ImportResult {
  total: number
  valid: number
  invalid: number
  rows: ImportRow[]
}

/** CSV 列顺序：设备名称,设备ID,所属总线,协议,从站ID,模板,地址 */
const CSV_HEADER = '设备名称,设备ID,所属总线,协议,从站ID,模板,地址'

/** 期望的列数（与 CSV_HEADER 对应） */
const EXPECTED_COLUMNS = 7

/** Modbus 协议名（唯一需要「从站ID」寻址的一类） */
const MODBUS_PROTOCOLS = ['modbus-tcp', 'modbus-rtu']

export function downloadTemplate(): void {
  const lines = [
    CSV_HEADER,
    '电力仪表-01,dev_meter_01,TCP-1,modbus-tcp,1,<模板名称>,',
    '温控器-02,dev_hvac_02,RTU-1,modbus-rtu,2,<模板名称>,',
  ]
  // BOM 由 downloadCsvText 统一附加，保证 Excel 打开中文不乱码
  downloadCsvText(lines.join('\n'), '设备批量导入模板.csv')
}

/**
 * 解析 CSV 文本为导入行。
 *
 * 列数与表头不一致的行会被标记为无效并给出明确原因——
 * 旧实现把这类行按位置硬取，用户看到的是「协议须为 modbus-tcp」这种
 * 完全误导的报错，看不出自己的文件被切坏了。
 */
export function parseCSV(rawText: string): ImportRow[] {
  const parsed = parseCsvRows(rawText)
  const rows: ImportRow[] = []
  // 第 1 行为表头，数据从第 2 行开始
  for (let i = 1; i < parsed.length; i++) {
    const { line, fields } = parsed[i]!
    const row: ImportRow = {
      line,
      name: fields[0] ?? '',
      id: fields[1] ?? '',
      bus: fields[2] ?? '',
      protocol: fields[3] ?? '',
      slaveId: fields[4] ?? '',
      template: fields[5] ?? '',
      address: fields[6] ?? '',
      valid: true,
    }
    if (fields.length !== EXPECTED_COLUMNS) {
      // 列数不符：多半是字段里含逗号没加引号（如「电表,1号」）
      row.valid = false
      row.error = `列数应为 ${EXPECTED_COLUMNS}（实际 ${fields.length}）；字段含逗号时需用双引号包裹`
    }
    rows.push(row)
  }
  return rows
}

/**
 * 逐行校验。
 *
 * @param rows       parseCSV 的结果
 * @param templates  网关模板库（可空：拉取失败时跳过模板相关校验，交给后端兜底）
 */
export function validateImport(rows: ImportRow[], templates: TemplateInfo[] = []): ImportResult {
  const seenIds = new Set<string>()
  const knownTemplates = new Map(templates.map((t) => [t.name, t]))
  for (const r of rows) {
    const problems: string[] = []
    // 解析阶段已判定的列数错误（多半是字段含逗号没加引号）优先保留：
    // 它是根因，后面的校验报错都是被切列导致的连带现象
    if (r.error) problems.push(r.error)
    if (!r.name) problems.push('缺少设备名称')
    if (!r.id) problems.push('缺少设备ID')
    else if (seenIds.has(r.id)) problems.push(`设备ID「${r.id}」重复`)
    else seenIds.add(r.id)
    if (!r.bus) problems.push('缺少所属总线')
    if (!r.protocol) problems.push('缺少协议')
    if (!r.template) {
      problems.push('缺少设备模板（后端要求每台设备绑定模板，模板决定可读点位）')
    } else if (templates.length > 0) {
      const tpl = knownTemplates.get(r.template)
      if (!tpl) {
        problems.push(`模板「${r.template}」不存在，可用模板：${templates.map((t) => t.name).join('、')}`)
      } else if (tpl.protocol && tpl.protocol !== r.protocol) {
        problems.push(`模板「${r.template}」的协议是 ${tpl.protocol}，与填写的 ${r.protocol} 不一致`)
      }
    }

    const isModbus = MODBUS_PROTOCOLS.includes(r.protocol)
    const sid = Number(r.slaveId)
    if (isModbus) {
      // Modbus 必须靠从站地址寻址，沿用原来的 1-247 范围校验
      if (!Number.isInteger(sid) || sid < 1 || sid > 247) {
        problems.push(`从站ID须为 1-247 的整数（当前：${r.slaveId || '空'}）`)
      } else {
        r.slaveId = sid
        // Modbus 的「地址」就是从站号，留空时自动补上，与设备表单的生成规则一致
        if (!r.address) r.address = String(sid)
      }
    } else {
      // 非 Modbus 协议（DLT645 / CJT188 等）用表地址寻址，从站ID 可空
      if (String(r.slaveId).trim() === '') {
        r.slaveId = ''
      } else if (!Number.isInteger(sid) || sid < 0 || sid > 255) {
        problems.push(`从站ID须为 0-255 的整数（当前：${r.slaveId}）`)
      } else {
        r.slaveId = sid
      }
      if (!r.address) {
        problems.push(`协议 ${r.protocol} 需要填写表地址（第 7 列），不能留空`)
      }
    }

    if (problems.length > 0) {
      r.valid = false
      r.error = problems.join('；')
    }
  }
  const valid = rows.filter((r) => r.valid).length
  return { total: rows.length, valid, invalid: rows.length - valid, rows }
}
