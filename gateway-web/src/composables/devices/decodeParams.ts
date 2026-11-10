/**
 * 数值解码参数的可选项（数据类型 / 字节序）。
 *
 * 点位表单与批量修改弹窗共用同一份清单：两处各写一份时，
 * 新增类型只在其中一处出现，会导致「表单能选、批量改不了」这类不一致。
 */

/** Modbus 数值类型：与后端 `PointDataType` 的规范名一致（也是实时数据里显示的类型名） */
export const DATA_TYPES = ['u16', 'i16', 'u32', 'i32', 'u64', 'i64', 'f32', 'f64']

/** 字节序：短码是后端主格式，long-* 为历史兼容写法 */
export const BYTE_ORDERS = [
  { value: 'ABCD', label: 'ABCD（大端）' },
  { value: 'DCBA', label: 'DCBA（小端）' },
  { value: 'CDAB', label: 'CDAB（字交换）' },
  { value: 'BADC', label: 'BADC（字节交换）' },
  { value: 'big-endian', label: 'big-endian（兼容写法）' },
  { value: 'little-endian', label: 'little-endian（兼容写法）' },
]
