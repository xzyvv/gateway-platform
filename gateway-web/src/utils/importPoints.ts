import type { PointConfig } from '@/api/types'

/**
 * 解析并校验批量导入的点位 JSON 文本
 * @returns { valid: PointConfig[]; error?: string }
 *   - 解析失败/非数组/无有效点位时返回 error
 *   - valid 为通过校验（含 id 字段）的点位列表
 *
 * Bug 2.9 (2026-Q1) 修复：原实现仅检查 id 字段存在，不校验字段类型；
 * 用户粘贴 `interval: "abc"`、`bus: 123` 等错误数据也能通过前端校验，
 * 提交后端才报错，用户体验差。改为逐字段类型校验，错误信息带
 * 字段路径，定位到具体哪个点位哪一列。
 */
export function parseImportPoints(text: string): { valid: PointConfig[]; error?: string } {
  const trimmed = text.trim()
  if (!trimmed) {
    return { valid: [], error: '请输入 JSON 数组' }
  }
  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return { valid: [], error: 'JSON 格式错误，请检查' }
  }
  if (!Array.isArray(parsed)) {
    return { valid: [], error: '请输入 JSON 数组' }
  }
  const valid: PointConfig[] = []
  const errors: string[] = []
  parsed.forEach((p: unknown, idx: number) => {
    const result = validateSinglePoint(p, idx)
    if (result.ok) {
      valid.push(result.point!)
    } else {
      errors.push(result.error!)
    }
  })
  if (valid.length === 0) {
    return { valid: [], error: errors[0] || '未找到有效点位' }
  }
  if (errors.length > 0) {
    // 部分有效：返回有效点位 + 警告信息（让用户知道有 N 条被忽略）
    return { valid, error: `${errors.length} 条点位校验失败（已忽略），示例: ${errors[0]}` }
  }
  return { valid }
}

interface PointValidationOk {
  ok: true
  point: PointConfig
}
interface PointValidationErr {
  ok: false
  error: string
}
type PointValidation = PointValidationOk | PointValidationErr

/**
 * 单个点位字段类型校验
 * - id/name/bus/protocol/address: 必填字符串
 * - interval/slave_id/address_num/function_code/register_count/ct_ratio/pt_ratio:
 *     可选 number（缺省 = 后端默认）
 * - scale/offset/deadband: 可选 number
 * - on_change: 可选 boolean
 * - byte_order/data_type/unit/formula/data_id: 可选 string
 */
function validateSinglePoint(p: unknown, idx: number): PointValidation {
  if (!p || typeof p !== 'object') {
    return { ok: false, error: `第 ${idx + 1} 条不是合法对象` }
  }
  const obj = p as Record<string, unknown>
  const path = `第 ${idx + 1} 条`

  // 必填字符串字段
  for (const k of ['id', 'bus', 'protocol', 'address'] as const) {
    if (typeof obj[k] !== 'string' || !(obj[k] as string).trim()) {
      return { ok: false, error: `${path} ${k} 字段必填且必须是非空字符串` }
    }
  }
  // name 可选但若是 string 不能为空
  if (obj.name != null && (typeof obj.name !== 'string' || !(obj.name as string).trim())) {
    return { ok: false, error: `${path} name 字段若提供则必须是非空字符串` }
  }

  // interval 必填 number >= 1
  if (typeof obj.interval !== 'number' || !Number.isFinite(obj.interval) || obj.interval < 1) {
    return { ok: false, error: `${path} interval 字段必填且必须是 >= 1 的数字` }
  }

  // 数值字段可选但若提供必须是 number
  const numFields: Array<keyof PointConfig> = [
    'slave_id', 'address_num', 'function_code', 'register_count',
    'ct_ratio', 'pt_ratio', 'scale', 'offset', 'deadband',
    'alarm_high', 'alarm_low', 'modbus_register',
  ]
  for (const k of numFields) {
    if (obj[k as string] != null && (typeof obj[k as string] !== 'number' || !Number.isFinite(obj[k as string] as number))) {
      return { ok: false, error: `${path} ${k} 字段若提供必须是数字` }
    }
  }

  // Modbus slave_id 范围 1-247
  if (typeof obj.slave_id === 'number' && (obj.slave_id < 1 || obj.slave_id > 247)) {
    return { ok: false, error: `${path} slave_id 必须在 1-247 范围（当前 ${obj.slave_id}）` }
  }

  // 字符串字段可选但若提供必须是 string
  const strFields: Array<keyof PointConfig> = [
    'data_id', 'data_type', 'byte_order', 'unit', 'formula', 'name', 'device_id',
  ]
  for (const k of strFields) {
    if (obj[k as string] != null && typeof obj[k as string] !== 'string') {
      return { ok: false, error: `${path} ${k} 字段若提供必须是字符串` }
    }
  }

  // on_change 必填 boolean
  if (typeof obj.on_change !== 'boolean') {
    return { ok: false, error: `${path} on_change 字段必填且必须是布尔值` }
  }

  return { ok: true, point: p as PointConfig }
}
