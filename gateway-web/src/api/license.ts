/**
 * License 授权 API（§8 GET /api/license + 授权管理 POST /api/license/import）
 *
 * 后端返回裸 LicenseSnapshot JSON（状态 + 本机设备码视图，非 ApiResponse 包装），
 * http 拦截器原样透传；类型来自 ts-rs 自动生成（api/generated），与后端结构自动同步。
 */
import { request } from './http'
import type { LicenseSnapshot } from './generated/LicenseSnapshot'

export type { LicenseState } from './generated/LicenseState'
export type { LicenseStatus } from './generated/LicenseStatus'
export type { LicenseSnapshot } from './generated/LicenseSnapshot'
export type { LocalFingerprints } from './generated/LocalFingerprints'

/**
 * 拉取当前授权状态快照（含本机设备码，供授权管理对话框展示/复制）。
 * 调用时机：登录成功后（auth store）、Layout 挂载时、授权管理对话框打开时。
 * 网关未就绪 / 旧版本无此端点时 reject，调用方自行静默兜底。
 */
export function fetchLicense(): Promise<LicenseSnapshot> {
  return request<LicenseSnapshot>({ method: 'GET', url: '/api/license' })
}

/**
 * 导入授权文件内容（.lic 文本）。
 * - 校验失败（签名/指纹/格式）：后端返回 400 + 面向客户的中文 message，本函数 reject；
 * - 校验通过：落盘并即时生效，resolve 最新快照（过期文件也会落盘，状态降级呈现）。
 */
export function importLicense(content: string): Promise<LicenseSnapshot> {
  return request<LicenseSnapshot>({
    method: 'POST',
    url: '/api/license/import',
    data: { content },
  })
}
