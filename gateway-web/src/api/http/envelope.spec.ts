/**
 * 统一信封豁免路径（isBareResponse / BARE_RESPONSE_PATHS）测试。
 *
 * 白名单必须与后端 `gateway-api/src/web/middleware.rs::is_envelope_exempt` 保持一致：
 * 命中则响应**不**做 ApiResponse 解包，body 原样返回调用方。
 */
import { describe, it, expect } from 'vitest'
import { isBareResponse, BARE_RESPONSE_PATHS } from './envelope'

describe('isBareResponse（豁免信封解包的路径）', () => {
  it('/health 及其子路径豁免（探针读裸 JSON）', () => {
    expect(isBareResponse('/health')).toBe(true)
    expect(isBareResponse('/health/ready')).toBe(true)
    expect(isBareResponse('/health/live')).toBe(true)
  })

  it('白名单内的四个路径豁免', () => {
    expect(isBareResponse('/api/metrics')).toBe(true)
    expect(isBareResponse('/api/history/export')).toBe(true)
    expect(isBareResponse('/api/auth/login')).toBe(true)
    expect(isBareResponse('/api/diagnostics/logs/download')).toBe(true)
  })

  it('带 query 的豁免路径仍命中（先剥 query 再比对）', () => {
    expect(isBareResponse('/api/metrics?format=prom')).toBe(true)
    expect(isBareResponse('/health/ready?probe=1')).toBe(true)
  })

  it('普通业务路径不豁免', () => {
    expect(isBareResponse('/api/status')).toBe(false)
    expect(isBareResponse('/api/device/list')).toBe(false)
    expect(isBareResponse('/api/metrics/extra')).toBe(false)
  })

  it('undefined / 空串不豁免', () => {
    expect(isBareResponse(undefined)).toBe(false)
    expect(isBareResponse('')).toBe(false)
  })

  it('BARE_RESPONSE_PATHS 与后端约定一致（不含 /health，探针路径单独判定）', () => {
    expect(BARE_RESPONSE_PATHS).toEqual([
      '/api/metrics',
      '/api/history/export',
      '/api/auth/login',
      '/api/diagnostics/logs/download',
    ])
    expect(BARE_RESPONSE_PATHS).not.toContain('/health')
  })
})
