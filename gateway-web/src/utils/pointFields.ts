/**
 * 点位 CSV 字段清单：导出与导入共用的唯一口径。
 *
 * 为什么要有这张表：
 * - 导出（点位页「批量导出」）与导入（批量导入页 CSV）必须用同一套列名与
 *   转换规则，否则「导出 → 改 → 导回」会错位；
 * - 每个字段都要钉死「留空」语义：
 *   - 可清空字段留空 = 清空该字段（如 unit / formula / alarm_high）；
 *   - 不可清空字段留空 = 保留原值（interval / scale / offset / deadband），
 *     这几个尤其危险：填 0 会把测点值乘成 0，绝不能让「空单元格」变成 0；
 * - `aggregation` 是嵌套结构（AggregationConfig），CSV 无法平铺，故不进清单，
 *   需要改聚合请走点位表单或整份配置导入。
 */
import type { PointConfig } from '@/api/types'

/** 单元格值的类型口径 */
export type PointFieldKind = 'text' | 'number' | 'bool'

/** 单个可导出 / 可导入字段的描述 */
export interface PointFieldSpec {
  /** CSV 列名（导出表头、导入定位都用它） */
  column: string
  /** 对应的 PointConfig 字段名 */
  key: keyof PointConfig
  kind: PointFieldKind
  /** 留空是否表示「清空该字段」；false 表示留空保留原值 */
  clearable: boolean
  /** 必填列：列存在但留空即校验失败 */
  required?: boolean
}

/** 精简档：与历史「点位 CSV 模板」逐列一致，Excel 里改地址/变比最顺手 */
export const POINT_FIELDS_BASIC: PointFieldSpec[] = [
  { column: 'id', key: 'id', kind: 'text', clearable: false, required: true },
  { column: 'name', key: 'name', kind: 'text', clearable: true },
  { column: 'device_id', key: 'device_id', kind: 'text', clearable: true },
  { column: 'bus', key: 'bus', kind: 'text', clearable: false, required: true },
  { column: 'protocol', key: 'protocol', kind: 'text', clearable: false, required: true },
  { column: 'address', key: 'address', kind: 'text', clearable: false, required: true },
  { column: 'slave_id', key: 'slave_id', kind: 'number', clearable: true },
  { column: 'address_num', key: 'address_num', kind: 'number', clearable: true },
  { column: 'ct_ratio', key: 'ct_ratio', kind: 'number', clearable: true },
  { column: 'pt_ratio', key: 'pt_ratio', kind: 'number', clearable: true },
]

/** 完整档：除嵌套的 aggregation 外全部字段，用于「导出 → 改 → 原样导回」的闭环 */
export const POINT_FIELDS_FULL: PointFieldSpec[] = [
  ...POINT_FIELDS_BASIC,
  { column: 'data_id', key: 'data_id', kind: 'text', clearable: true },
  { column: 'function_code', key: 'function_code', kind: 'number', clearable: true },
  { column: 'data_type', key: 'data_type', kind: 'text', clearable: true },
  { column: 'byte_order', key: 'byte_order', kind: 'text', clearable: true },
  { column: 'register_count', key: 'register_count', kind: 'number', clearable: true },
  { column: 'interval', key: 'interval', kind: 'number', clearable: false },
  { column: 'scale', key: 'scale', kind: 'number', clearable: false },
  { column: 'offset', key: 'offset', kind: 'number', clearable: false },
  { column: 'formula', key: 'formula', kind: 'text', clearable: true },
  { column: 'deadband', key: 'deadband', kind: 'number', clearable: false },
  { column: 'on_change', key: 'on_change', kind: 'bool', clearable: true },
  { column: 'unit', key: 'unit', kind: 'text', clearable: true },
  { column: 'alarm_high', key: 'alarm_high', kind: 'number', clearable: true },
  { column: 'alarm_low', key: 'alarm_low', kind: 'number', clearable: true },
  { column: 'modbus_register', key: 'modbus_register', kind: 'number', clearable: true },
  { column: 'writable', key: 'writable', kind: 'bool', clearable: true },
]

/** 列名 → 字段描述（导入按列名定位，表头大小写不敏感） */
export const POINT_FIELD_BY_COLUMN: Map<string, PointFieldSpec> = new Map(
  POINT_FIELDS_FULL.map((f) => [f.column, f]),
)

/** 单元格解析结果 */
export type PointCellParse =
  /** `value` 为 undefined 表示「留空但不可清空」→ 保留原值 */
  | { ok: true; value: string | number | boolean | null | undefined }
  | { ok: false; error: string }

/**
 * 单元格文本 → 字段值。
 *
 * - 空单元格：可清空字段 → null（bool → false）；不可清空字段 → undefined（保留原值）
 * - 数字列：非数字即报错
 * - 布尔列：true/1/yes/是 视为真，其余为假
 */
export function parsePointCell(spec: PointFieldSpec, raw: string): PointCellParse {
  const text = raw.trim()
  if (text === '') {
    if (!spec.clearable) return { ok: true, value: undefined }
    return { ok: true, value: spec.kind === 'bool' ? false : null }
  }
  if (spec.kind === 'number') {
    const n = Number(text)
    if (!Number.isFinite(n)) return { ok: false, error: `${spec.column} 必须是数字` }
    return { ok: true, value: n }
  }
  if (spec.kind === 'bool') {
    const t = text.toLowerCase()
    return { ok: true, value: t === 'true' || t === '1' || t === 'yes' || t === '是' }
  }
  return { ok: true, value: text }
}

/** 字段值 → 单元格文本（导出用；空值导出成空串） */
export function formatPointCell(spec: PointFieldSpec, value: unknown): string {
  if (value === null || value === undefined) return ''
  if (spec.kind === 'bool') return value ? 'true' : 'false'
  return String(value)
}
