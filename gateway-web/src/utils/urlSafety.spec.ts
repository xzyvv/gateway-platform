import { describe, it, expect, vi, beforeEach } from 'vitest'
import {
  formatUrlSafetyError,
  validateOutboundUrl,
  validateOutboundUrlAfterDns,
} from './urlSafety'

// ============================================================
// W-14 修复：SSRF / 本地资源 / scheme 防护
// ============================================================

describe('validateOutboundUrl — 同步基本校验（W-14）', () => {
  it('合法 https URL 通过', () => {
    const r = validateOutboundUrl('https://example.com/hook')
    expect(r.ok).toBe(true)
  })

  it('合法 http URL 通过', () => {
    const r = validateOutboundUrl('http://example.com:8080/hook')
    expect(r.ok).toBe(true)
  })

  it('空字符串 → invalid-url', () => {
    const r = validateOutboundUrl('')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('invalid-url')
  })

  it('无 scheme 的相对 URL → invalid-url', () => {
    const r = validateOutboundUrl('example.com/hook')
    expect(r.ok).toBe(false)
  })

  it('完全乱码 → invalid-url', () => {
    const r = validateOutboundUrl('not a url at all !!!')
    expect(r.ok).toBe(false)
  })

  // ---- Scheme 白名单 ----

  it('file:// scheme → scheme-not-allowed', () => {
    const r = validateOutboundUrl('file:///etc/passwd')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('scheme-not-allowed')
  })

  it('javascript: scheme → scheme-not-allowed', () => {
    const r = validateOutboundUrl('javascript:alert(1)')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('scheme-not-allowed')
  })

  it('data: scheme → scheme-not-allowed', () => {
    const r = validateOutboundUrl('data:text/html,<script>alert(1)</script>')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('scheme-not-allowed')
  })

  it('ftp:// scheme → scheme-not-allowed', () => {
    const r = validateOutboundUrl('ftp://files.example.com/')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('scheme-not-allowed')
  })

  it('ws:// scheme → scheme-not-allowed（webhook 不应走 ws）', () => {
    const r = validateOutboundUrl('ws://example.com/socket')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('scheme-not-allowed')
  })

  // ---- Host 黑名单 ----

  it('localhost → host-blocked', () => {
    const r = validateOutboundUrl('http://localhost:8080/hook')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('LOCALHOST 大小写不敏感', () => {
    const r = validateOutboundUrl('http://LOCALHOST/hook')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('0.0.0.0 → host-blocked', () => {
    const r = validateOutboundUrl('http://0.0.0.0:80/')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('127.0.0.1 → host-blocked（loopback）', () => {
    const r = validateOutboundUrl('http://127.0.0.1/hook')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('127.0.0.5 任意 loopback → host-blocked', () => {
    const r = validateOutboundUrl('http://127.5.5.5/hook')
    expect(r.ok).toBe(false)
  })

  it('10.x 私网 → host-blocked', () => {
    const r = validateOutboundUrl('http://10.0.0.1/admin')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('172.16-31 私网 → host-blocked', () => {
    expect(validateOutboundUrl('http://172.16.0.1/').ok).toBe(false)
    expect(validateOutboundUrl('http://172.20.5.5/').ok).toBe(false)
    expect(validateOutboundUrl('http://172.31.255.255/').ok).toBe(false)
  })

  it('172.32+ 公网 → 通过', () => {
    // 172.32 不在 172.16/12 范围内，是公网
    const r = validateOutboundUrl('http://172.32.0.1/')
    expect(r.ok).toBe(true)
  })

  it('192.168.x 私网 → host-blocked', () => {
    const r = validateOutboundUrl('http://192.168.1.1/admin')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('169.254.x 链路本地 → host-blocked', () => {
    const r = validateOutboundUrl('http://169.254.169.254/latest/meta-data/')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('100.64-127 CGNAT → host-blocked', () => {
    const r = validateOutboundUrl('http://100.64.0.1/')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  it('云元数据 host → host-blocked', () => {
    const r = validateOutboundUrl('http://metadata.google.internal/')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('host-blocked')
  })

  // ---- IPv6 ----

  it('IPv6 ::1 → host-blocked', () => {
    const r = validateOutboundUrl('http://[::1]/hook')
    expect(r.ok).toBe(false)
  })

  it('IPv6 fe80:: link-local → host-blocked', () => {
    const r = validateOutboundUrl('http://[fe80::1]/hook')
    expect(r.ok).toBe(false)
  })

  it('IPv6 fc00:: ULA → host-blocked', () => {
    const r = validateOutboundUrl('http://[fc00::1]/hook')
    expect(r.ok).toBe(false)
  })

  it('IPv6 ::ffff:127.0.0.1 IPv4-mapped → host-blocked', () => {
    const r = validateOutboundUrl('http://[::ffff:127.0.0.1]/hook')
    expect(r.ok).toBe(false)
  })

  it('IPv6 公网地址 → 通过', () => {
    // 2001:db8:: 是文档示例（实际公网 IPv6 不会是这个）
    const r = validateOutboundUrl('http://[2001:db8::1]/hook')
    expect(r.ok).toBe(true)
  })

  // ---- 公网 ----

  it('公网域名 → 通过', () => {
    const r = validateOutboundUrl('https://hooks.slack.com/services/T0/B0/XXX')
    expect(r.ok).toBe(true)
  })

  it('公网 IP 1.1.1.1 → 通过', () => {
    const r = validateOutboundUrl('http://1.1.1.1/')
    expect(r.ok).toBe(true)
  })

  it('公网 IP 8.8.8.8 → 通过', () => {
    const r = validateOutboundUrl('https://8.8.8.8:443/')
    expect(r.ok).toBe(true)
  })

  // ---- trustedHosts 豁免 ----

  it('trustedHosts 精确匹配内网 IP → 放行', () => {
    const r = validateOutboundUrl('http://192.168.1.10/hook', {
      trustedHosts: ['192.168.1.10'],
    })
    expect(r.ok).toBe(true)
  })

  it('trustedHosts 子域匹配 → 放行', () => {
    const r = validateOutboundUrl('https://webhook.trusted.example.com/hook', {
      trustedHosts: ['.trusted.example.com'],
    })
    expect(r.ok).toBe(true)
  })

  it('trustedHosts 不匹配 → 仍被拒', () => {
    const r = validateOutboundUrl('http://192.168.1.99/hook', {
      trustedHosts: ['192.168.1.10'],
    })
    expect(r.ok).toBe(false)
  })
})

describe('validateOutboundUrlAfterDns — 仅静态校验（#14：已移除前端 DNS 解析）', () => {
  beforeEach(() => {
    vi.resetModules()
  })

  it('IP 字面 host → 放行', async () => {
    const r = await validateOutboundUrlAfterDns('https://8.8.8.8/')
    expect(r.ok).toBe(true)
  })

  it('域名不再做 DNS 解析 → 放行（SSRF 防护由真正出网点负责）', async () => {
    // #14: 浏览器/Tauri WebView 无 node:dns，旧实现会把所有域名 webhook 误判为
    // dns-failed 而拒绝。即便把 node:dns mock 成必定抛错，也不应影响放行。
    vi.doMock('node:dns', () => {
      throw new Error('node:dns must not be used in the browser build')
    })
    const r = await validateOutboundUrlAfterDns('https://example.com/hook')
    expect(r.ok).toBe(true)
    if (r.ok) expect(r.resolvedIps).toEqual([])
  })

  it('域名形式的内网字面量仍被基本校验拦截', async () => {
    const r = await validateOutboundUrlAfterDns('http://192.168.1.99/hook')
    expect(r.ok).toBe(false)
  })

  it('基本校验失败时直接返回（scheme 非法）', async () => {
    const r = await validateOutboundUrlAfterDns('file:///etc/passwd')
    expect(r.ok).toBe(false)
    if (!r.ok) expect(r.error.kind).toBe('scheme-not-allowed')
  })
})

describe('formatUrlSafetyError — 错误消息（W-14）', () => {
  it('invalid-url 消息含 reason', () => {
    const msg = formatUrlSafetyError({ kind: 'invalid-url', reason: 'empty host' })
    expect(msg).toContain('empty host')
  })

  it('scheme-not-allowed 消息含 scheme', () => {
    const msg = formatUrlSafetyError({ kind: 'scheme-not-allowed', scheme: 'file:' })
    expect(msg).toContain('file:')
  })

  it('host-blocked 消息含 host + reason', () => {
    const msg = formatUrlSafetyError({
      kind: 'host-blocked',
      host: 'localhost',
      reason: 'loopback',
    })
    expect(msg).toContain('localhost')
    expect(msg).toContain('loopback')
  })

  it('ip-blocked 消息含 ip + reason', () => {
    const msg = formatUrlSafetyError({
      kind: 'ip-blocked',
      ip: '10.0.0.1',
      reason: 'private',
    })
    expect(msg).toContain('10.0.0.1')
    expect(msg).toContain('private')
  })

  it('dns-failed 消息含 reason', () => {
    const msg = formatUrlSafetyError({ kind: 'dns-failed', reason: 'ENOTFOUND' })
    expect(msg).toContain('ENOTFOUND')
  })
})
