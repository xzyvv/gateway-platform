/**
 * useGatewayDiscovery — 局域网自动发现网关
 *
 * 场景：笔记本通过网线直连工业网关（同 /24 子网），前端启动后无需手填 IP，
 * 自动扫描候选地址并 ping /health/live，命中即列入候选列表。
 *
 * 设计取舍：
 * - 选 HTTP 主动扫描而非 mDNS / SSDP：零新依赖、跨平台、网线直连 / 调试网必通。
 *   mDNS 需 Bonjour（Windows 默认装但有兼容问题），SSDP 多播易被企业防火墙拦；
 *   HTTP 主动扫 /24 子网 254 个 IP，平均 30-60ms × 254 = 8-15 秒可接受（debug 场景一次性）。
 * - 并发控制：16 个并发 fetch，避免浏览器 TCP 队列拥堵。
 * - 超时：单 IP 1.5s，整体扫描 8s 强截止；用户可中途点"重新扫描"。
 * - 命中标准：`/health/live` 返回 200 + body 形如 `{"status":"ok"}` 或 `{"status":"live"}`
 *   （对应 gateway-core 的 health_live handler）。
 *
 * 兼容性：
 * - 仅在 Tauri 桌面端 + Vite 开发服务器场景使用（直连调试）；
 * - 远程部署场景不会调用本模块（远程有公网 IP，扫局域网无意义）。
 */

import { ref, type Ref } from 'vue'
import { isTauri, scanGatewayLan } from '@/api'
import { extractErrorMessage } from '@/utils/errorHandler'

export interface DiscoveredGateway {
  /** 形如 http://192.168.1.100:8080 */
  url: string
  /** 响应耗时（ms），用于候选列表展示"网络质量" */
  latencyMs: number
  /** 是否为首选（同一子网内 IP 末尾 .1 视为路由器，.254 常见网关默认，最后响应最快） */
  preferred?: boolean
}

const SCAN_TIMEOUT_MS = 1500          // 单 IP 超时
const SCAN_BUDGET_MS = 8000           // 整体扫描硬截止
const SCAN_CONCURRENCY = 16           // 并发数
const COMMON_PORTS = [8080, 80, 8000] // 候选端口顺序


/**
 * 并发扫描一组 IP，返回命中的网关候选。
 * 端口按 COMMON_PORTS 顺序试，第一个命中即停。
 */
async function probeHosts(
  hosts: string[],
  signal: AbortSignal,
  onProgress?: (found: DiscoveredGateway[]) => void,
): Promise<DiscoveredGateway[]> {
  const found: DiscoveredGateway[] = []
  let cursor = 0
  const startTime = performance.now()

  async function worker() {
    while (cursor < hosts.length && !signal.aborted) {
      if (performance.now() - startTime > SCAN_BUDGET_MS) return
      const idx = cursor++
      const host = hosts[idx]
      for (const port of COMMON_PORTS) {
        if (signal.aborted) return
        const url = `http://${host}:${port}/health/live`
        const t0 = performance.now()
        const ctrl = new AbortController()
        const timeout = setTimeout(() => ctrl.abort(), SCAN_TIMEOUT_MS)
        // 链接外层 signal：组件卸载时立即停
        const onAbort = () => ctrl.abort()
        signal.addEventListener('abort', onAbort, { once: true })
        try {
          const resp = await fetch(url, {
            method: 'GET',
            signal: ctrl.signal,
            // 阻止浏览器缓存
            cache: 'no-store',
            mode: 'cors',
          })

          if (!resp.ok) continue
          const body = await resp.text()
          // 健康检查返回形如 {"status":"live"} 或 {"status":"ok"}
          // gateway-core 的 health_live 返回的是 Json {"status":"live"}
          if (!body.includes('"status"')) continue
          const latency = Math.round(performance.now() - t0)
          found.push({ url: `http://${host}:${port}`, latencyMs: latency })
          onProgress?.([...found])
          break // 命中即停（同 IP 不同端口不再试）
        } catch {
          // 超时 / 网络错 / abort — 静默忽略，继续下一个
        } finally {
          // C3: 确保异常路径也清理定时器和事件监听，避免泄漏
          clearTimeout(timeout)
          signal.removeEventListener('abort', onAbort)
        }
      }
    }
  }

  // 启动 N 个并发 worker
  const workers = Array.from({ length: SCAN_CONCURRENCY }, () => worker())
  await Promise.all(workers)
  return found
}

/**
 * 用 WebRTC 拿本机所有 IPv4（RTCPeerConnection + 假 STUN 反射本机 IP）。
 * 拿不到就 fallback 到常见子网（192.168.1/24, 192.168.0/24）。
 */
async function detectLocalIPs(): Promise<string[]> {
  return new Promise((resolve) => {
    const RTC = (window as typeof window & { RTCPeerConnection?: typeof RTCPeerConnection })
      .RTCPeerConnection
    if (!RTC) {
      resolve(fallbackSubnets())
      return
    }
    const pc = new RTC({ iceServers: [] })
    pc.createDataChannel('')
    // C3: 用 done() 统一收口，确保 fallback timer 在成功路径被清除
    let resolved = false
    let fallbackTimer: ReturnType<typeof setTimeout> | null = null
    function done(ips: string[]) {
      if (resolved) return
      resolved = true
      if (fallbackTimer) clearTimeout(fallbackTimer)
      try { pc.close() } catch { /* ignore */ }
      resolve(ips)
    }
    pc.onicecandidate = (e: RTCPeerConnectionIceEvent) => {
      if (!e || !e.candidate || !e.candidate.candidate) {
        done(fallbackSubnets())
        return
      }
      const parts = e.candidate.candidate.split(' ')
      const ip4 = parts[4]
      // 过滤 IPv6
      if (ip4 && ip4.includes('.') && !ip4.startsWith('127.')) {
        const parts2 = ip4.split('.')
        if (parts2.length === 4) {
          const subnet = `${parts2[0]}.${parts2[1]}.${parts2[2]}`
          const ips: string[] = []
          for (let i = 1; i < 255; i++) ips.push(`${subnet}.${i}`)
          done(ips)
        }
      }
    }
    pc.createOffer().then((offer: RTCSessionDescriptionInit) => pc.setLocalDescription(offer)).catch(() => {
      done(fallbackSubnets())
    })
    // 兜底超时：3 秒拿不到就 fallback
    fallbackTimer = setTimeout(() => {
      done(fallbackSubnets())
    }, 3000)
  })
}

function fallbackSubnets(): string[] {
  // 工业现场常见子网：192.168.1.x（笔记本/网关默认），192.168.0.x（小路由器）
  // 也覆盖一些 PLC 常用的 10.x 段
  const segments = [
    '192.168.1', '192.168.0', '10.0.0', '10.10.0',
  ]
  const ips: string[] = []
  for (const seg of segments) {
    for (let i = 1; i < 255; i++) ips.push(`${seg}.${i}`)
  }
  return ips
}

export interface UseGatewayDiscoveryReturn {
  scanning: Ref<boolean>
  found: Ref<DiscoveredGateway[]>
  /** 总耗时（ms），扫描完成后填入 */
  elapsedMs: Ref<number>
  /** 上次错误信息（如有） */
  error: Ref<string | null>
  /** 触发一次扫描；返回最终命中的网关列表 */
  scan: () => Promise<DiscoveredGateway[]>
  /** 取消当前扫描 */
  cancel: () => void
}

export function useGatewayDiscovery(): UseGatewayDiscoveryReturn {
  const scanning = ref(false)
  const found = ref<DiscoveredGateway[]>([])
  const elapsedMs = ref(0)
  const error = ref<string | null>(null)
  let controller: AbortController | null = null

  async function scan(): Promise<DiscoveredGateway[]> {
    if (scanning.value) return found.value
    // Tauri 桌面端：用 Rust 原生扫描，避免 webview 直连 HTTP（CSP 收紧后必需）
    if (isTauri()) {
      scanning.value = true
      found.value = []
      error.value = null
      elapsedMs.value = 0
      const t0 = performance.now()
      try {
        const result = await scanGatewayLan(8080)
        found.value = result.map((g) => ({
          url: `http://${g.ip}:${g.port}`,
          latencyMs: g.latency_ms,
          preferred: g.ip.endsWith('.1') || g.ip.endsWith('.254'),
        }))
        elapsedMs.value = Math.round(performance.now() - t0)
      } catch (e) {
        error.value = extractErrorMessage(e)
      } finally {
        scanning.value = false
      }
      return found.value
    }
    controller?.abort()
    controller = new AbortController()
    scanning.value = true
    found.value = []
    error.value = null
    elapsedMs.value = 0
    const t0 = performance.now()
    try {
      const ips = await detectLocalIPs()
      if (controller.signal.aborted) return []
      // 优先扫 .1 / .254 / .100（常见默认地址）
      const priority = ['.1', '.254', '.100', '.10']
      const priorityIps: string[] = []
      const remainingIps: string[] = []
      const seen = new Set<string>()
      for (const ip of ips) {
        const tail = ip.split('.').pop()
        if (tail && priority.includes('.' + tail) && !seen.has(ip)) {
          priorityIps.push(ip)
          seen.add(ip)
        } else {
          remainingIps.push(ip)
        }
      }
      const ordered = [...priorityIps, ...remainingIps]
      const result = await probeHosts(ordered, controller.signal, (live) => {
        found.value = live
      })
      // 按延迟排序（小的在前），同名 url 去重
      const uniq = new Map<string, DiscoveredGateway>()
      for (const g of result) {
        const existing = uniq.get(g.url)
        if (!existing || existing.latencyMs > g.latencyMs) uniq.set(g.url, g)
      }
      const sorted = Array.from(uniq.values()).sort((a, b) => a.latencyMs - b.latencyMs)
      found.value = sorted
      elapsedMs.value = Math.round(performance.now() - t0)
      return sorted
    } catch (e) {
      error.value = extractErrorMessage(e)
      return []
    } finally {
      scanning.value = false
    }
  }

  function cancel() {
    controller?.abort()
    scanning.value = false
  }

  return { scanning, found, elapsedMs, error, scan, cancel }
}
