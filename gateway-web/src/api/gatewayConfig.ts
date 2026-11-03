/**
 * 网关配置缓存 + Tauri axios 适配器
 *
 * Tauri 桌面端通过 invoke 调用 Rust 端的 proxy_request 转发 HTTP，
 * 绕开浏览器 CORS；Web 端走默认 xhr 适配器。
 *
 * tauriAdapter 与 cachedConfig 紧耦合（需要 active 网关 URL），
 * 因此放同一文件，由 http.ts 导入用作 axios adapter。
 */
import type { AxiosRequestConfig, AxiosResponse } from 'axios'
import { isTauri } from './env'
// 静态引用在飞注册中心：inflight.ts 不反向依赖本模块，无循环依赖；
// 此前为绕开 gatewayConfig ↔ http 循环而用动态 import('./http')，
// 但 http.ts 已被多处静态引入，动态导入零拆包收益并触发构建警告。
import { triggerGatewaySwitchRefresh } from './inflight'

export interface GatewayEntry {
  name: string
  url: string
}

export interface GatewayConfig {
  gateways: GatewayEntry[]
  active: string | null
}

let cachedConfig: GatewayConfig | null = null
let configPromise: Promise<GatewayConfig> | null = null

/** 获取激活的网关基础 URL（如 http://192.168.1.100:8080），Web 模式返回空串 */
export function getGatewayBaseUrl(): string {
  if (!isTauri() || !cachedConfig) return ''
  const active = cachedConfig.active
  if (!active) return ''
  const gw = cachedConfig.gateways.find((g) => g.name === active)
  return gw ? gw.url.replace(/\/$/, '') : ''
}

/**
 * 当前激活的网关名（未配置 / Web 模式返回 null）。
 * 用于 localStorage 多网关命名空间隔离（见 utils/gatewayStorage.ts）。
 */
export function getActiveGatewayName(): string | null {
  return cachedConfig?.active ?? null
}

/** 从 Tauri 后端加载网关配置并缓存 */
export async function loadGatewayConfig(): Promise<GatewayConfig> {
  if (!isTauri()) {
    cachedConfig = { gateways: [], active: null }
    return cachedConfig
  }
  if (cachedConfig) return cachedConfig
  if (configPromise) return configPromise
  configPromise = (async () => {
    try {
      const { invoke } = await import('@tauri-apps/api/core')
      const cfg = await invoke<GatewayConfig>('get_config')
      cachedConfig = cfg
      return cfg
    } catch {
      cachedConfig = { gateways: [], active: null }
      return cachedConfig
    }
  })()
  return configPromise
}

/** 切换激活的网关并刷新缓存 */
export async function setActiveGateway(name: string): Promise<void> {
  if (!isTauri()) return
  const { invoke } = await import('@tauri-apps/api/core')
  await invoke('set_active_gateway', { name })
  cachedConfig = await invoke<GatewayConfig>('get_config')
  // 切网关后刷新页面：先 abort 所有在飞请求（防打到旧网关），再 reload 加载新网关上下文
  triggerGatewaySwitchRefresh()
}

/** 保存网关配置 */
export async function saveGatewayConfig(config: GatewayConfig): Promise<void> {
  if (!isTauri()) return
  const { invoke } = await import('@tauri-apps/api/core')
  await invoke('save_config', { config })
  cachedConfig = config
}

/** 将 axios 请求通过 Tauri invoke 转发到网关，绕开 CORS */
export async function tauriAdapter(config: AxiosRequestConfig): Promise<AxiosResponse> {
  // 网关未配置预检：避免 axios 把请求转发到 proxy_request 后由 Rust 端再抛异常
  if (!cachedConfig?.active) {
    throw new Error('网关地址未设置，请在登录页填写后重试')
  }
  const { invoke } = await import('@tauri-apps/api/core')
  const method = (config.method || 'get').toUpperCase()
  const path = config.url || ''

  // 提取请求头为普通对象
  const headers: Record<string, string> = {}
  if (config.headers) {
    for (const [k, v] of Object.entries(config.headers)) {
      if (v !== undefined && v !== null) {
        headers[k] = String(v)
      }
    }
  }

  // 请求体：axios 已经做了 JSON 序列化，这里直接透传
  let body: unknown = undefined
  if (config.data !== undefined && config.data !== null) {
    if (typeof config.data === 'string') {
      try {
        body = JSON.parse(config.data)
      } catch {
        body = config.data
      }
    } else {
      body = config.data
    }
  }

  const resp = await invoke<{ status: number; body: string }>('proxy_request', {
    method,
    path,
    body,
    headers,
  })

  // 解析响应体
  let data: unknown = resp.body
  try {
    data = JSON.parse(resp.body)
  } catch {
    // 非 JSON，保留原始字符串
  }

  return {
    data,
    status: resp.status,
    statusText: resp.status >= 200 && resp.status < 300 ? 'OK' : 'Error',
    headers: {},
    config,
  } as AxiosResponse
}

/** 扫描到的网关信息 */
export interface DiscoveredGateway {
  ip: string
  port: number
  name: string
  latency_ms: number
}

/** 扫描局域网内的网关设备（仅 Tauri 桌面端可用） */
export async function scanGatewayLan(port?: number): Promise<DiscoveredGateway[]> {
  if (!isTauri()) return []
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<DiscoveredGateway[]>('scan_gateway_lan', { port: port ?? 8080 })
}

/** opt-6: 激活网关健康检测结果（直连启动检测） */
export interface GatewayHealth {
  reachable: boolean
  latency_ms: number
  error: string | null
}

/**
 * opt-6: 检测激活网关健康状态（仅 Tauri 桌面端可用）。
 *
 * desktop 启动 bootstrap 后调用：若不可达，提示用户检查地址或扫描局域网，
 * 避免"登录后才发现网关连不上"的死锁体验。Web 模式返回 null（health badge 自行轮询）。
 */
export async function checkGatewayHealth(): Promise<GatewayHealth | null> {
  if (!isTauri()) return null
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<GatewayHealth>('check_gateway_health')
}

/** 检测任意 URL 的健康状态（切网关前探测目标可达性，仅 Tauri） */
export async function checkUrlHealth(url: string): Promise<GatewayHealth | null> {
  if (!isTauri()) return null
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke<GatewayHealth>('check_url_health', { url })
}

/**
 * 转发 webhook 请求到外部 URL（避免 webview 直连 HTTP，仅 Tauri）
 *
 * W-14 修复：调用前必须做 URL 安全校验。Tauri 模式下 Rust 进程代为
 * 发送请求，URL 解析在内核完成，绕过浏览器同源策略——必须前端先拦截
 * 明显恶意 URL（file:// / localhost / 127.0.0.1 / 169.254.169.254 等）。
 *
 * 双端校验已对齐（不再有"后续安全审计"遗留）：
 * - 前端：静态校验（scheme 白名单 + host 黑名单字面量），拦误填与明显恶意 URL
 * - Rust 端：`forward_webhook` 命令内部强制调用 `url_safety::validate_outbound_url`
 *   同口径校验，并走独立的强制 TLS 校验客户端（Bug-D2 修复），
 *   绕过前端直接 invoke 同样会被拦截
 */
export async function forwardWebhook(url: string, body: string): Promise<void> {
  if (!isTauri()) return
  // #14: 前端只做静态校验（scheme 白名单 + host 黑名单字面量）。原先额外调用
  // validateOutboundUrlAfterDns 做 "DNS rebinding 防护"，但浏览器/WebView 无 node:dns，
  // 该调用会把所有域名 webhook 误判为 dns-failed 而拒绝；且 SSRF 防护必须在真正的
  // 出网点做（Tauri 的 forward_webhook 命令 / 网关北向），前端 DNS 校验无防护价值。
  const { validateOutboundUrl, formatUrlSafetyError } = await import('@/utils/urlSafety')
  const basic = validateOutboundUrl(url)
  if (!basic.ok) {
    throw new Error(`webhook url blocked: ${formatUrlSafetyError(basic.error)}`)
  }
  const { invoke } = await import('@tauri-apps/api/core')
  await invoke('forward_webhook', { url, body })
}
