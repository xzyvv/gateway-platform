/**
 * API Token 管理 — 加密存储版
 *
 * Token 通过 AES-GCM 加密后存到 localStorage，
 * 加密密钥存在 IndexedDB（不同沙箱，XSS 提取难度更高）。
 *
 * 加密细节见 [secureStorage.ts]。
 *
 * 调用方使用方式：
 * - 写操作（setToken / clearToken）：可直接 await 或 fire-and-forget
 * - 读操作（getToken）：必须 await，axios 拦截器已适配
 */
import { getToken as getSecureToken, setToken as setSecureToken, clearToken as clearSecureToken } from '../utils/secureStorage'

export async function getToken(): Promise<string> {
  return getSecureToken()
}

export async function setToken(token: string): Promise<void> {
  await setSecureToken(token)
}

export function clearToken(): void {
  clearSecureToken()
}

/**
 * 启动时调用一次：把可能残留的明文老 token 自动加密迁移。
 * 详见 secureStorage.migrateLegacyToken。
 */
export async function migrateLegacyToken(): Promise<boolean> {
  const { migrateLegacyToken: m } = await import('../utils/secureStorage')
  return m()
}
