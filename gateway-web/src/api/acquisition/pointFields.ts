import { request } from '../index'

/**
 * 点位字段字典（`GET /api/point-fields` 的 data）。
 *
 * 取值与后端校验读同一份常量（`VALID_BYTE_ORDERS` / `DATA_TYPE_NAMES` /
 * `RECOMMENDED_UNITS`），前端只保留离线兜底，不再维护第二份清单。
 */
export interface PointFieldDictionary {
  /** 统一数值解码的规范类型名（u16 / i16 / … / f64，不含别名） */
  data_types: string[]
  /** 合法字节序（短四字码 + 历史长写法） */
  byte_orders: string[]
  /** 推荐工程单位 */
  units: string[]
}

export function getPointFields(): Promise<PointFieldDictionary> {
  return request<PointFieldDictionary>({ url: '/api/point-fields', method: 'GET' })
}
