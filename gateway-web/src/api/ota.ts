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
    // 固件可达 64MB，全局 15s 超时在 4G/慢链路上必然先于上传完成触发，
    // 会让运维看到「升级失败」而后端可能已收到部分请求体。上传独占长超时。
    timeout: 10 * 60 * 1000,
  })
}

/** OTA 回滚 */
export function otaRollback(): Promise<{ status: string }> {
  return request({ url: '/api/ota/rollback', method: 'POST' })
}
