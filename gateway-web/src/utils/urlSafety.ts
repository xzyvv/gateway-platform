/**
 * URL 安全校验 — W-14 修复：SSRF / 本地资源 / 危险 scheme 防护
 *
 * 旧实现 `forwardWebhook(url, body)` 与 `useWebhook.setUrl(url)` 都不校验
 * URL——任何 `file:///etc/passwd`、`http://127.0.0.1:22`、`http://192.168.1.1/admin`
 * 都会被透传到 Tauri 后端或浏览器 fetch。
 * 攻击场景：
 * - Tauri 模式下，Rust 进程代为发送请求，URL 解析在内核完成，绕过浏览器同源策略
 * - Web 模式下，`fetch` 走浏览器 CORS 看起来安全，但 fetch 默认 mode 是 cors
 *   时跨源 POST 会被预检（preflight）挡住；mode: 'no-cors' 时虽能发但**浏览器仍会
 *   拦截 file:// 等 scheme**——但 `setUrl` 不校验会让用户在不知情时把 SSRF 风险
 *   引入系统
 * - 用户在配置 webhook URL 时可能误填内网 IP（NAS、PLC 管理页），导致告警 webhook
 *   把变更事件发到内网设备
 *
 * 修复策略（OWASP SSRF 防护指南，best-effort 前端层）：
 * 1. **Scheme 白名单**：仅 http:// / https://
 * 2. **Host 黑名单**：localhost / 127.0.0.0/8 / ::1 / 0.0.0.0 / 169.254.0.0/16 / 10/8 /
 *    172.16/12 / 192.168/16 / 100.64/10（RFC1918 + 链路本地 + CGNAT）
 * 3. **Host 元数据黑名单**：`metadata.google.internal` / `169.254.169.254`（云元数据）
 * 4. **显式配置 trustedHosts** 走白名单豁免（如 `192.168.1.10` 是用户显式信任的
 *    内网 webhook 接收方）
 *
 * 注意（#14 后续修正）：**前端不再做 DNS 解析后 IP 校验**——浏览器 / Tauri WebView 中
 * `node:dns` 不可用，且 SSRF 防护必须在真正的出网点做。前端只做静态校验（字面量层）。
 *
 * Rust 端已对齐：`gateway-desktop/src-tauri/src/url_safety.rs` 实现了同口径的
 * scheme 白名单 + host/IP 黑名单，由 `forward_webhook` 命令在出网前强制调用
 * （Bug-D2 修复），并使用独立的强制 TLS 校验客户端。因此绕过前端直接调
 * `invoke('forward_webhook', ...)` 同样会被拦截——不再需要"后续安全审计"跟进。
 */

export type UrlSafetyError =
  | { kind: 'invalid-url'; reason: string }
  | { kind: 'scheme-not-allowed'; scheme: string }
  | { kind: 'host-blocked'; host: string; reason: string }
  | { kind: 'ip-blocked'; ip: string; reason: string }
  | { kind: 'dns-failed'; reason: string }

export interface UrlSafetyOptions {
  /**
   * 用户显式信任的 host 列表（精确匹配或后缀匹配 `.trusted.example.com`）。
   * 即使 host 在黑名单（如内网 IP），命中 trusted 仍放行。
   * 默认空数组（内网 IP 一律拒绝）。
   */
  trustedHosts?: ReadonlyArray<string>
  /**
   * 是否允许私有 IP（10/8, 172.16/12, 192.168/16, 127/8, ::1, 169.254/16, 100.64/10）。
   * 默认 false。设为 true 仅在开发/调试环境——生产必须保持 false。
   */
  allowPrivateIps?: boolean
}

/** 黑名单：精确 host（lowercase） */
const BLOCKED_HOSTS: ReadonlySet<string> = new Set([
  'localhost',
  '0.0.0.0',
  '::',
  '::1',
  'metadata.google.internal',
  '169.254.169.254', // AWS / GCP / Azure 元数据
  'metadata',
  'metadata.internal',
])

/** 私有 / 链路本地 / CGNAT / IPv6 ULA / IPv6 link-local IPv4-mapped */
function isPrivateOrReservedIp(ip: string): { blocked: true; reason: string } | { blocked: false } {
  // IPv4
  const ipv4 = ip.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/)
  if (ipv4 && ipv4[1] !== undefined) {
    const a = Number(ipv4[1])
    const b = Number(ipv4[2])
    if (a === 127) return { blocked: true, reason: 'loopback (127.0.0.0/8)' }
    if (a === 10) return { blocked: true, reason: 'private (10.0.0.0/8)' }
    if (a === 172 && b >= 16 && b <= 31) return { blocked: true, reason: 'private (172.16.0.0/12)' }
    if (a === 192 && b === 168) return { blocked: true, reason: 'private (192.168.0.0/16)' }
    if (a === 169 && b === 254) return { blocked: true, reason: 'link-local (169.254.0.0/16)' }
    if (a === 100 && b >= 64 && b <= 127) return { blocked: true, reason: 'CGNAT (100.64.0.0/10)' }
    if (a === 0) return { blocked: true, reason: 'unspecified (0.0.0.0)' }
    if (a >= 224) return { blocked: true, reason: 'multicast/reserved (224.0.0.0/4)' }
    return { blocked: false }
  }
  // IPv6
  if (ip === '::' || ip === '::1') return { blocked: true, reason: 'IPv6 loopback / unspecified' }
  if (ip.startsWith('fe80:')) return { blocked: true, reason: 'IPv6 link-local (fe80::/10)' }
  if (ip.startsWith('fc') || ip.startsWith('fd')) return { blocked: true, reason: 'IPv6 ULA (fc00::/7)' }
  // IPv4-mapped IPv6（::ffff:a.b.c.d 或 ::ffff:7f00:1 规范形式）——保守地全部 deny。
  // 这类地址在内网部署中可能被绕过（攻击者用 IPv6 mapped 形式访问 IPv4 内网服务），
  // 正常 webhoook 不会用 IPv4-mapped。
  if (ip.startsWith('::ffff:')) return { blocked: true, reason: 'IPv4-mapped IPv6 (::ffff:0:0/96)' }
  return { blocked: false }
}

/**
 * Host 字符串层黑名单：精确匹配或常见本地名
 * （已 resolved 的 host 不需要这一步——直接看 IP）
 */
function isBlockedHostLiteral(host: string): { blocked: true; reason: string } | { blocked: false } {
  // Node URL.hostname 对 IPv6 保留方括号（[fc00::1]），
  // 校验前先剥除——避免 startsWith('fc') 永远 false。
  const stripped = host.startsWith('[') && host.endsWith(']') ? host.slice(1, -1) : host
  const lower = stripped.toLowerCase()
  if (BLOCKED_HOSTS.has(lower)) {
    if (lower === 'localhost' || lower === '0.0.0.0') return { blocked: true, reason: 'loopback / unspecified' }
    if (lower === '::' || lower === '::1') return { blocked: true, reason: 'IPv6 loopback / unspecified' }
    if (lower === 'metadata.google.internal') return { blocked: true, reason: 'cloud metadata endpoint' }
    if (lower === '169.254.169.254') return { blocked: true, reason: 'cloud metadata (link-local 169.254.169.254)' }
    if (lower === 'metadata' || lower === 'metadata.internal') return { blocked: true, reason: 'metadata endpoint' }
  }
  // 形如 127.x.x.x / 10.x.x.x / 192.168.x.x 字面量（含 IPv4-mapped IPv6 递归）
  const ipCheck = isPrivateOrReservedIp(lower)
  if (ipCheck.blocked) return ipCheck
  return { blocked: false }
}

/**
 * Host 是否被 trustedHosts 列表豁免
 */
function isTrustedHost(host: string, trustedHosts: ReadonlyArray<string>): boolean {
  const lower = host.toLowerCase()
  return trustedHosts.some((trusted) => {
    const t = trusted.toLowerCase()
    if (t === lower) return true
    if (t.startsWith('.')) return lower.endsWith(t) || lower === t.slice(1)
    return false
  })
}

/**
 * 纯 URL 解析层校验（同步，不做 DNS 查询）。
 * 仅在 host 字面命中黑名单时返回 Err。
 * 适用于：
 * - 浏览器 fetch（Tauri / Web 模式都可用）
 * - 用户配置 setUrl 时的即时反馈（DNS 解析较慢，先做字面校验）
 */
export function validateOutboundUrl(
  url: string,
  options: UrlSafetyOptions = {},
): { ok: true; url: URL } | { ok: false; error: UrlSafetyError } {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch (e) {
    return {
      ok: false,
      error: { kind: 'invalid-url', reason: e instanceof Error ? e.message : String(e) },
    }
  }

  // 1. Scheme 白名单
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
    return { ok: false, error: { kind: 'scheme-not-allowed', scheme: parsed.protocol } }
  }

  const host = parsed.hostname
  if (!host) {
    return { ok: false, error: { kind: 'invalid-url', reason: 'empty host' } }
  }

  // 2. Host 黑名单（精确字面）
  const hostCheck = isBlockedHostLiteral(host)
  if (hostCheck.blocked && !isTrustedHost(host, options.trustedHosts ?? [])) {
    return { ok: false, error: { kind: 'host-blocked', host, reason: hostCheck.reason } }
  }

  // 3. IP 字面（私网 / 链路本地 / CGNAT）已在 isBlockedHostLiteral 中处理，
  //    此处无需重复判定；DNS 解析后的 IP 校验前端已移除（见文件头注释）。

  return { ok: true, url: parsed }
}

/**
 * 出站 URL 的"完整校验"入口。
 *
 * #14: 已**移除**前端 DNS 解析校验。原因：
 *  - 浏览器 / Tauri WebView 中 `node:dns` 并不可用（旧实现的动态 import 会抛错），
 *    导致所有域名形式的 webhook 地址一律被判 `dns-failed` 而拒绝，功能不可用；
 *  - 前端做 DNS 校验对 SSRF / DNS-rebinding 没有防护价值：出网发生在服务端，
 *    攻击者控制的是服务端的解析结果，而非浏览器的。SSRF 防护必须在真正的出网点
 *    （网关北向 / Tauri 的 `forward_webhook` 命令）实现。
 *
 * 本函数现在等价于 `validateOutboundUrl`（scheme 白名单 + host 黑名单字面量），
 * 保留函数名以维持既有调用方与导出稳定；`resolvedIps` 恒为空数组。
 */
export async function validateOutboundUrlAfterDns(
  url: string,
  options: UrlSafetyOptions = {},
): Promise<{ ok: true; url: URL; resolvedIps: string[] } | { ok: false; error: UrlSafetyError }> {
  const basic = validateOutboundUrl(url, options)
  if (!basic.ok) return basic
  return { ok: true, url: basic.url, resolvedIps: [] }
}

/** 把 UrlSafetyError 转成给用户看的中文/英文消息 */
export function formatUrlSafetyError(err: UrlSafetyError): string {
  switch (err.kind) {
    case 'invalid-url':
      return `URL 格式非法: ${err.reason}`
    case 'scheme-not-allowed':
      return `URL scheme 不允许（仅支持 http/https）: ${err.scheme}`
    case 'host-blocked':
      return `目标主机被安全策略阻止: ${err.host} (${err.reason})`
    case 'ip-blocked':
      return `目标 IP 命中内网/保留段: ${err.ip} (${err.reason})`
    case 'dns-failed':
      return `DNS 解析失败: ${err.reason}`
  }
}
