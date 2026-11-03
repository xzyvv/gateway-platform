/**
 * 协议工具 API（barrel 重新导出）
 *
 * 后端实现：[../../../gateway-api/src/web/api/api_protocol_tools/]
 * 类型由 ts-rs 生成：../generated/<Type>.ts
 *
 * 模块拆分：
 * - ./constants：静态常量（PROTO_LABEL）+ 字面量联合类型
 * - ./types：View 接口（camelCase，与 mock 接口兼容）
 * - ./converters：Raw → View 转换函数（snake_case → camelCase，bigint → number）
 * - ./autoDetect / ./packetCapture / ... ：按协议工具子域拆分的 API 函数
 *
 * 本文件作为 barrel 重新导出，向后兼容所有 `from '@/api/protocolTools'` 调用方。
 * 新代码应按需从子文件导入，避免引入未使用符号。
 */
export * from './constants'
export * from './types'
export * from './autoDetect'
export * from './packetCapture'
export * from './packetEditor'
export * from './packetReplay'
export * from './convertRules'
export * from './registerScan'
export * from './slaveScan'
export * from './slaveSimulator'
