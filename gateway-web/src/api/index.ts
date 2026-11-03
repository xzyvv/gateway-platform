/**
 * API 入口（barrel）
 *
 * 模块拆分：
 * - ./env：Tauri 环境检测 + Mock 开关
 * - ./token：API Token 管理（localStorage 持久化）
 * - ./gatewayConfig：Tauri 网关配置缓存 + axios 适配器（绕开 CORS）
 * - ./http：axios 实例 + 请求/响应拦截器 + request 包装
 *
 * 本文件作为 barrel 重新导出，向后兼容所有 `from '@/api'` / `from './index'` 调用方。
 * 各业务 API（devices.ts / points.ts / ...）继续从 './index' 导入 request。
 */
export * from './env'
export * from './token'
export * from './gatewayConfig'
export * from './http'