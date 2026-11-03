/**
 * 证书管理 API
 *
 * 路由（与后端 gateway-api/src/web/api/api_system/handlers/certificates.rs 对齐）：
 * - GET  /api/certificates        -> listCertificates
 * - POST /api/certificates/upload -> uploadCertificate
 * - POST /api/certificates/renew  -> renewCertificate
 *
 * 第 1 期冗余收口：原 `GET /api/cert/status` 与本列表同源（同一 CertManager），
 * 后端已删除该端点，其字段（`days_remaining` / `cert_path`）并入列表项。
 */
import { request } from './index'

export interface Certificate {
  id: string
  name: string
  cert_type: 'server' | 'client' | 'ca'
  subject: string
  issuer: string
  serial_number: string
  valid_from: number
  valid_to: number
  /** 剩余有效天数（负数 = 已过期） */
  days_remaining: number
  /** 证书文件在本机的路径 */
  cert_path: string
  fingerprint: string
  enabled: boolean
  bound_to?: string
}

export function listCertificates(): Promise<Certificate[]> {
  return request<Certificate[]>({ url: '/api/certificates', method: 'GET' })
}

export function uploadCertificate(name: string, certPem: string, keyPem: string): Promise<void> {
  return request<void>({
    url: '/api/certificates/upload',
    method: 'POST',
    data: { name, cert_pem: certPem, key_pem: keyPem },
  })
}

export function renewCertificate(): Promise<void> {
  return request<void>({ url: '/api/certificates/renew', method: 'POST' })
}