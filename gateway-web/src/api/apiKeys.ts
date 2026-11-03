/**
 * API 密钥（网关真实后端：/api/access/api-keys）
 *
 * 安全模型：后端只存 SHA-256 摘要与预览（前 8 后 4），完整 Key 仅在创建时
 * 返回一次（CreatedApiKey.key），此后任何接口都拿不到——页面须在创建成功后
 * 立即展示并引导用户复制保存。
 *
 * 鉴权语义：读接口任何启用密钥可用；写接口要求密钥至少持有一个 `*:write`
 * 或 `*` scope，否则 403。密钥停用/过期后调用返回 401（与错误 token 同形，防枚举）。
 */
import { request } from './index'

export interface ApiKey {
  id: string
  name: string
  /** 密钥预览（前 8 后 4），非完整凭证，仅用于辨认 */
  keyPreview: string
  scopes: string[]
  enabled: boolean
  expiresAt?: number
  createdAt: number
  lastUsedAt?: number
  callCount: number
}

/** 创建成功响应：完整 Key 仅此一次返回 */
export interface CreatedApiKey extends ApiKey {
  key: string
}

/**
 * 可授予的权限范围（必须与后端 `access/handlers.rs::ALL_SCOPES`、
 * `docs/api-access.md` 的 scope 词表一致）。
 *
 * 系统域写端点（用户 / API 密钥 / 系统设置 / 诊断）只接受主 api_token，
 * 故不提供 `system:write`：否则密钥可自建用户或高权密钥，变相提权。
 */
const ALL_SCOPES = [
  'devices:read',
  'devices:write',
  'points:read',
  'points:write',
  'config:read',
  'config:write',
  'history:read',
]

/**
 * 返回 scope 词表的**副本**。
 *
 * 直接返回模块级数组会让调用方的 `push`/`splice` 永久污染全局常量
 * （页面里 `allScopes` 常用于绑定复选框，误改会波及所有后续调用）。
 */
export function getAllScopes(): string[] {
  return [...ALL_SCOPES]
}

export function listApiKeys(): Promise<ApiKey[]> {
  return request({ url: '/api/access/api-keys', method: 'GET' })
}

export function createApiKey(
  name: string,
  scopes: string[],
  expiresDays?: number,
): Promise<CreatedApiKey> {
  return request({
    url: '/api/access/api-keys',
    method: 'POST',
    data: { name, scopes, expiresDays },
  })
}

// id 必须转义：与 `watchdog.restartService` 一致。未编码的 `/`、`?`、`#`
// 会被当成路径分隔符或查询串，导致打到错误的路由（甚至改写路径）。
export function setApiKeyEnabled(id: string, enabled: boolean): Promise<void> {
  return request({
    url: `/api/access/api-keys/${encodeURIComponent(id)}`,
    method: 'PUT',
    data: { enabled },
  })
}

export function deleteApiKey(id: string): Promise<void> {
  return request({ url: `/api/access/api-keys/${encodeURIComponent(id)}`, method: 'DELETE' })
}
