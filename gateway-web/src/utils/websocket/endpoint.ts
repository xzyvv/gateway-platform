import { getGatewayBaseUrl, getToken, isTauri } from '@/api'

/**
 * 构造 WebSocket 连接 URL。
 *
 * B1 修复：token 不再拼到 URL query，避免落 Nginx / 反代 access log、浏览器历史、DevTools。
 * 浏览器原生 WebSocket 不能设 Authorization 头，但可以通过第二参数传入子协议协商。
 * 与后端约定：使用 `bearer.<token>` 作为 Sec-WebSocket-Protocol 子协议值。
 * 注：WebSocket 子协议不允许包含 `,` / 控制字符；token 走 base64url/hex 时天然安全。
 */
export async function buildWsUrl(path: string): Promise<string> {
  if (isTauri()) {
    const baseUrl = getGatewayBaseUrl()
    if (!baseUrl) {
      throw new Error('网关地址未设置，请在登录页填写后重试')
    }
    const wsProto = baseUrl.startsWith('https:') ? 'wss:' : 'ws:'
    const host = baseUrl.replace(/^https?:\/\//, '')
    return `${wsProto}//${host}${path}`
  }
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:'
  return `${proto}//${window.location.host}${path}`
}

/**
 * 构造 WebSocket 子协议数组：包含 `bearer.<token>`（与后端 extract_token_from_ws_protocol 对齐）。
 *
 * 子协议拼接规则：WebSocket 规范禁止子协议值含 `,`，且仅允许可见 ASCII（`!`-`~`）。
 * token 字符集由后端 `extract_token_from_ws_protocol` 校验（`is_ascii_graphic` + 长度 ≤ 512）。
 * 前端在拼接前再做一次白名单过滤：移除逗号 / 控制字符 / 长度截断，避免握手被服务端拒。
 */
export async function buildWsProtocols(): Promise<string[]> {
  const token = await getToken()
  if (!token) return []
  // 严格字符集白名单：仅保留 base64url/hex 常用字符
  const safe = token
    .split('')
    .filter((c) => {
      const code = c.charCodeAt(0)
      return code >= 0x21 && code <= 0x7e && c !== ','
    })
    .join('')
  if (!safe) return []
  const sub = `bearer.${safe}`
  // 子协议总长上限 512（与后端 MAX_PROTOCOL_LEN 对齐）
  if (sub.length > 512) return []
  return [sub]
}
