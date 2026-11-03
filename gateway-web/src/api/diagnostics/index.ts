/**
 * 诊断运维 API（聚合 raw + view-friendly 适配层）
 *
 * 路由：
 * - GET  /api/diagnostics/bus-load              -> listBusLoads
 * - GET  /api/diagnostics/bus-load/:bus          -> getBusLoad
 * - POST /api/diagnostics/bus-load/refresh        -> refreshBusLoads
 * - GET  /api/diagnostics/modbus-errors          -> listModbusErrors
 * - GET  /api/diagnostics/modbus-errors/issues      -> listCommonIssues
 * - GET  /api/diagnostics/stats                   -> getStatsReport
 * - GET  /api/diagnostics/schedule-timeline       -> getScheduleTimeline
 * - POST /api/diagnostics/task-diag/:pointId       -> diagnosePoint
 * - GET  /api/diagnostics/gateway                 -> getGatewayDiag
 *
 * 后端实现：[../../../../gateway-api/src/web/api/api_diagnostics/]
 * 类型由 ts-rs 生成：../generated/<Type>.ts
 *
 * 模块拆分：
 * - ./raw：raw API + ts-rs 类型重导出（snake_case + bigint）
 * - ./view：View-friendly 适配层（camelCase + number 时间戳 + CSV 导出）
 *
 * 本文件作为 barrel 重新导出，向后兼容所有 `from '@/api/diagnostics'` 调用方。
 * 新代码应按需从子文件导入，避免引入未使用符号。
 */
export * from './raw'
export * from './view'
