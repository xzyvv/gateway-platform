/**
 * 数值解码参数的可选项（数据类型 / 字节序）。
 *
 * 点位表单与批量修改弹窗共用同一份清单：两处各写一份时，
 * 新增类型只在其中一处出现，会导致「表单能选、批量改不了」这类不一致。
 *
 * 取值集的唯一来源是后端（`GET /api/point-fields`，读 `DATA_TYPE_NAMES` /
 * `VALID_BYTE_ORDERS`），这里的常量只是离线 / 旧后端时的兜底。登录后由
 * `loadPointFields` 原地替换，因此导出的是 `reactive` 数组而非普通常量。
 */

import { reactive } from 'vue'

/** 兜底数据类型：与后端 `PointDataType` 的规范名一致（也是实时数据里显示的类型名） */
const FALLBACK_DATA_TYPES = ['u16', 'i16', 'u32', 'i32', 'u64', 'i64', 'f32', 'f64']

/** 字节序下拉项 */
export interface ByteOrderOption {
  value: string
  label: string
}

/**
 * 兜底字节序：短码是后端主格式，long-* 为历史兼容写法。
 *
 * 8 个取值与后端 `VALID_BYTE_ORDERS` 一一对应；后端下发时按本表重排，
 * 后端新增的取值追加在后面（label 暂用原值），保证「下拉里不会缺值」。
 */
const FALLBACK_BYTE_ORDERS: ByteOrderOption[] = [
  { value: 'ABCD', label: 'ABCD（大端）' },
  { value: 'DCBA', label: 'DCBA（小端）' },
  { value: 'CDAB', label: 'CDAB（字交换）' },
  { value: 'BADC', label: 'BADC（字节交换）' },
  { value: 'big-endian', label: 'big-endian（兼容写法）' },
  { value: 'little-endian', label: 'little-endian（兼容写法）' },
  { value: 'big-endian-byte-swapped', label: 'big-endian-byte-swapped（兼容写法）' },
  { value: 'little-endian-byte-swapped', label: 'little-endian-byte-swapped（兼容写法）' },
]

/** 当前生效的数据类型选项（登录后由后端字典原地替换） */
export const DATA_TYPES: string[] = reactive([...FALLBACK_DATA_TYPES])

/** 当前生效的字节序选项（登录后由后端字典原地替换） */
export const BYTE_ORDERS: ByteOrderOption[] = reactive(
  FALLBACK_BYTE_ORDERS.map((o) => ({ ...o })),
)

/** 用后端字典替换数据类型选项；空列表时保留兜底 */
export function applyDataTypes(values: string[]): void {
  const clean = values.map((v) => String(v).trim()).filter(Boolean)
  if (clean.length === 0) return
  DATA_TYPES.splice(0, DATA_TYPES.length, ...clean)
}

/**
 * 用后端字典替换字节序选项。
 *
 * 已知取值按本地顺序 + 本地文案排列（顺序稳定、说明完整）；
 * 后端新增的取值追加在末尾，label 暂用原值，避免「存了新值但下拉里没有」。
 */
export function applyByteOrders(values: string[]): void {
  const clean = values.map((v) => String(v).trim()).filter(Boolean)
  if (clean.length === 0) return
  const known = FALLBACK_BYTE_ORDERS.filter((o) => clean.includes(o.value)).map((o) => ({ ...o }))
  const extra = clean
    .filter((v) => !known.some((o) => o.value === v))
    .map((v) => ({ value: v, label: v }))
  BYTE_ORDERS.splice(0, BYTE_ORDERS.length, ...known, ...extra)
}

/**
 * 数值解码协议集（兜底值）。
 *
 * 启动后由 `GET /api/protocols` 的 `numeric_decode` 覆盖（见 `applyNumericDecodeProtocols`），
 * 这里只是离线/旧后端时的兜底，与后端 `config::validate::uses_numeric_decode` 一致。
 */
const FALLBACK_NUMERIC_DECODE = ['modbus', 'mc', 'fins', 'dlt645', 'cjt188']

/** 当前生效的数值解码协议集；允许运行期被后端描述符刷新 */
const numericDecodeProtocols = new Set(FALLBACK_NUMERIC_DECODE)

/**
 * 用后端协议描述符刷新数值解码协议集。
 *
 * 传空列表（旧后端没有该字段 / 解析异常）时保持兜底，避免把判断能力清空后
 * 表单突然不再露出「数据类型 / 字节序」。
 */
export function applyNumericDecodeProtocols(protocols: string[]): void {
  const names = protocols.map((p) => p.trim().toLowerCase()).filter(Boolean)
  if (names.length === 0) return
  numericDecodeProtocols.clear()
  for (const n of names) numericDecodeProtocols.add(n)
}

/**
 * 是否使用统一数值解码族（数据类型 + 字节序）。
 *
 * 协议集由后端注册表下发（`numeric_decode` 能力位），前端不再硬编码：
 * 新增协议只改后端 `uses_numeric_decode`，表单门控与字段校验自动跟上。
 * iec104（float/normalized/…）与 s7comm（word/int/real/…）自带独立语义，后端标记为 false。
 */
export function usesNumericDecode(protocol?: string | null): boolean {
  const p = (protocol ?? '').trim().toLowerCase()
  return numericDecodeProtocols.has(p)
}
