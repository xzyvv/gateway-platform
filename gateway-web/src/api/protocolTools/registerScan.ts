/**
 * 寄存器扫描 API
 * POST   /api/protocol-tools/register-scan
 * GET    /api/protocol-tools/register-scan
 * GET    /api/protocol-tools/register-scan/:id
 * DELETE /api/protocol-tools/register-scan/:id
 * POST   /api/protocol-tools/register-scan/:id/suggestions
 */
import { request } from '../index'
import type { RegType } from './constants'
import type { ScanResult, PointSuggestion } from './types'
import type { ScanResult as ScanResultRaw, PointSuggestion as PointSuggestionRaw } from '../generated'
import { toScanResult, toPointSuggestion } from './types'

export function startScan(bus: string, slaveId: number, regType: RegType, startAddr: number, endAddr: number): Promise<string> {
  return request<string>({ url: '/api/protocol-tools/register-scan', method: 'POST', data: { bus, slave_id: slaveId, reg_type: regType, start_addr: startAddr, end_addr: endAddr } })
}
export function getScanResult(id: string): Promise<ScanResult | null> {
  return request<ScanResultRaw | null>({ url: '/api/protocol-tools/register-scan/' + encodeURIComponent(id), method: 'GET' }).then((r) => r ? toScanResult(r) : null)
}
export function listScanResults(): Promise<ScanResult[]> {
  return request<ScanResultRaw[]>({ url: '/api/protocol-tools/register-scan', method: 'GET' }).then((arr) => arr.map(toScanResult))
}
export function deleteScanResult(id: string): Promise<void> {
  return request<void>({ url: '/api/protocol-tools/register-scan/' + encodeURIComponent(id), method: 'DELETE' })
}
export function generatePointSuggestions(r: ScanResult): Promise<PointSuggestion[]> {
  return request<PointSuggestionRaw[]>({ url: '/api/protocol-tools/register-scan/' + encodeURIComponent(r.id) + '/suggestions', method: 'POST' }).then((arr) => arr.map(toPointSuggestion))
}