import { request } from './index'
import type { OtaStatus } from './types'

/** OTA 状态 */
export function getOtaStatus(): Promise<OtaStatus> {
  return request<OtaStatus>({ url: '/api/ota/status', method: 'GET' })
}

/** OTA 上传固件 */
export function otaUpload(firmware: ArrayBuffer): Promise<{ status: string }> {
  return request({
    url: '/api/ota/upload',
    method: 'POST',
    data: firmware,
    headers: { 'Content-Type': 'application/octet-stream' },
  })
}

/** OTA 回滚 */
export function otaRollback(): Promise<{ status: string }> {
  return request({ url: '/api/ota/rollback', method: 'POST' })
}
