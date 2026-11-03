import { describe, it, expect, vi, beforeEach } from 'vitest'
import type { AxiosRequestConfig } from 'axios'

// Tauri invoke / 环境判定 / 切网关刷新均经 vi.hoisted 提前声明，供 vi.mock 工厂引用
const { invokeMock, isTauriMock, switchRefreshMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  isTauriMock: vi.fn(() => false),
  switchRefreshMock: vi.fn(),
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('./env', () => ({ isTauri: isTauriMock }))
vi.mock('./inflight', () => ({ triggerGatewaySwitchRefresh: switchRefreshMock }))

type GatewayConfigModule = typeof import('./gatewayConfig')

/**
 * gatewayConfig 内部有模块级缓存（cachedConfig），
 * 每个用例用 vi.resetModules() + 动态 import 拿一份全新实例，避免用例间串扰。
 */
async function loadModule(): Promise<GatewayConfigModule> {
  vi.resetModules()
  return await import('./gatewayConfig')
}

interface InvokeCall {
  cmd: string
  args?: Record<string, unknown>
}

function invokeCalls(): InvokeCall[] {
  return invokeMock.mock.calls.map((c) => ({ cmd: c[0] as string, args: c[1] as InvokeCall['args'] }))
}

describe('gatewayConfig（网关配置缓存 + Tauri 适配器契约）', () => {
  beforeEach(() => {
    invokeMock.mockReset()
    switchRefreshMock.mockReset()
    isTauriMock.mockReset()
    isTauriMock.mockReturnValue(false)
  })

  // Web 模式（无 Tauri 运行时）必须短路：不 invoke，直接返回空配置
  it('Web 模式 loadGatewayConfig 返回空配置且不调用 invoke', async () => {
    const mod = await loadModule()

    await expect(mod.loadGatewayConfig()).resolves.toEqual({ gateways: [], active: null })
    expect(invokeMock).not.toHaveBeenCalled()
  })

  // 锁定 Tauri 命令名：必须是 'get_config'（Rust 端 state.rs 注册名），
  // 误写成 'load_config' 会直接 invoke 失败并静默退化成空配置。
  it('Tauri 模式 loadGatewayConfig 调用 get_config 并原样返回配置', async () => {
    isTauriMock.mockReturnValue(true)
    const cfg = { gateways: [{ name: 'gw1', url: 'http://192.168.1.10:8080' }], active: 'gw1' }
    invokeMock.mockResolvedValue(cfg)
    const mod = await loadModule()

    await expect(mod.loadGatewayConfig()).resolves.toBe(cfg)
    expect(invokeMock).toHaveBeenCalledWith('get_config')
  })

  // 缓存契约：第二次调用不得重复 invoke（否则每次进页面都打一次 Rust）
  it('Tauri 模式 loadGatewayConfig 命中缓存后不再 invoke', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue({ gateways: [], active: null })
    const mod = await loadModule()

    await mod.loadGatewayConfig()
    await mod.loadGatewayConfig()

    expect(invokeMock).toHaveBeenCalledTimes(1)
  })

  // 错误分支：invoke 抛错时降级为空配置，不把异常抛给调用方（避免桌面端启动即崩）
  it('Tauri 模式 invoke 失败时 loadGatewayConfig 降级为空配置', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockRejectedValue(new Error('invoke failed'))
    const mod = await loadModule()

    await expect(mod.loadGatewayConfig()).resolves.toEqual({ gateways: [], active: null })
  })

  // 锁定 getGatewayBaseUrl 的三条空值分支：Web 模式 / 无缓存 / 无激活网关都必须返回空串，
  // 否则 axios baseURL 会变成 'undefined/api/...' 之类。
  it('getGatewayBaseUrl 在 Web 模式、无缓存、无激活网关时均返回空串', async () => {
    const mod = await loadModule()

    expect(mod.getGatewayBaseUrl()).toBe('')

    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue({ gateways: [{ name: 'gw1', url: 'http://1.2.3.4:8080' }], active: null })
    const mod2 = await loadModule()
    await mod2.loadGatewayConfig()
    expect(mod2.getGatewayBaseUrl()).toBe('')
  })

  // 锁定尾斜杠剥离：baseURL 拼路径时会出现 '//api/xxx'
  it('getGatewayBaseUrl 去掉网关地址的尾斜杠', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue({
      gateways: [{ name: 'gw1', url: 'http://192.168.1.10:8080/' }],
      active: 'gw1',
    })
    const mod = await loadModule()
    await mod.loadGatewayConfig()

    expect(mod.getGatewayBaseUrl()).toBe('http://192.168.1.10:8080')
  })

  // 锁定激活网关名：用于 localStorage 多网关命名空间隔离，取不到时必须是 null
  it('getActiveGatewayName 返回激活网关名，未加载时为 null', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue({
      gateways: [{ name: 'gw1', url: 'http://1.2.3.4:8080' }],
      active: 'gw1',
    })
    const mod = await loadModule()

    expect(mod.getActiveGatewayName()).toBeNull()
    await mod.loadGatewayConfig()
    expect(mod.getActiveGatewayName()).toBe('gw1')
  })

  // 锁定切换网关契约：先 invoke set_active_gateway{name}，再重新拉 get_config，
  // 最后触发刷新（abort 在飞请求 + reload），顺序错了会打到旧网关。
  it('setActiveGateway 依次调用 set_active_gateway / get_config 并触发刷新', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_config') return { gateways: [{ name: 'gw2', url: 'http://1.2.3.4' }], active: 'gw2' }
      return undefined
    })
    const mod = await loadModule()

    await mod.setActiveGateway('gw2')

    const calls = invokeCalls()
    expect(calls.map((c) => c.cmd)).toEqual(['set_active_gateway', 'get_config'])
    expect(calls[0]!.args).toEqual({ name: 'gw2' })
    expect(switchRefreshMock).toHaveBeenCalledTimes(1)
  })

  // Web 模式切换网关必须是空操作（不能 invoke）
  it('Web 模式 setActiveGateway 不调用 invoke', async () => {
    const mod = await loadModule()
    await mod.setActiveGateway('gw1')
    expect(invokeMock).not.toHaveBeenCalled()
  })

  // 锁定保存配置契约：invoke save_config{config}，并同步更新本地缓存
  it('saveGatewayConfig 调用 save_config 并更新缓存', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue(undefined)
    const mod = await loadModule()

    const cfg = { gateways: [{ name: 'gw1', url: 'http://1.2.3.4' }], active: 'gw1' }
    await mod.saveGatewayConfig(cfg)

    expect(invokeMock).toHaveBeenCalledWith('save_config', { config: cfg })
    expect(mod.getActiveGatewayName()).toBe('gw1')
    expect(mod.getGatewayBaseUrl()).toBe('http://1.2.3.4')
  })

  // 网关未配置时的预检：必须在 invoke 之前抛中文提示，
  // 否则错误会变成 Rust 侧的 'NO_ACTIVE_GATEWAY' 英文码。
  it('tauriAdapter 在未配置激活网关时抛出中文提示且不 invoke', async () => {
    const mod = await loadModule()

    await expect(mod.tauriAdapter({ url: '/api/config', method: 'get' })).rejects.toThrow(
      '网关地址未设置，请在登录页填写后重试',
    )
    expect(invokeMock).not.toHaveBeenCalled()
  })

  // 锁定转发契约：method 大写、path 取 url、body 字符串 JSON 反序列化、headers 字符串化
  it('tauriAdapter 以大写 method、url 作为 path 调用 proxy_request', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_config') return { gateways: [{ name: 'gw1', url: 'http://1.2.3.4' }], active: 'gw1' }
      return { status: 200, body: '{"ok":true}' }
    })
    const mod = await loadModule()
    await mod.loadGatewayConfig()

    const resp = await mod.tauriAdapter({
      url: '/api/config',
      method: 'post',
      data: JSON.stringify({ a: 1 }),
      headers: { 'X-Test': 1, 'X-Skip': undefined },
    } as unknown as AxiosRequestConfig)

    const proxy = invokeCalls().find((c) => c.cmd === 'proxy_request')!
    expect(proxy.args!.method).toBe('POST')
    expect(proxy.args!.path).toBe('/api/config')
    expect(proxy.args!.body).toEqual({ a: 1 })
    expect(proxy.args!.headers).toEqual({ 'X-Test': '1' })
    expect(resp.data).toEqual({ ok: true })
    expect(resp.status).toBe(200)
    expect(resp.statusText).toBe('OK')
  })

  // 边界：method 缺省时按 GET 处理；响应体非 JSON 时保留原始字符串；非 2xx 时 statusText 为 Error
  it('tauriAdapter 缺省 method 用 GET，非 JSON 响应保留原文，非 2xx 时 statusText 为 Error', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'get_config') return { gateways: [{ name: 'gw1', url: 'http://1.2.3.4' }], active: 'gw1' }
      return { status: 503, body: 'not json' }
    })
    const mod = await loadModule()
    await mod.loadGatewayConfig()

    const resp = await mod.tauriAdapter({ url: '/api/config' } as AxiosRequestConfig)

    const proxy = invokeCalls().find((c) => c.cmd === 'proxy_request')!
    expect(proxy.args!.method).toBe('GET')
    expect(proxy.args!.body).toBeUndefined()
    expect(resp.data).toBe('not json')
    expect(resp.statusText).toBe('Error')
  })

  // 锁定局域网扫描契约：默认端口 8080（Rust 端 scan.rs 的默认端口一致），可显式覆盖
  it('scanGatewayLan 默认端口 8080，可显式指定端口', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue([])
    const mod = await loadModule()

    await mod.scanGatewayLan()
    expect(invokeMock).toHaveBeenLastCalledWith('scan_gateway_lan', { port: 8080 })

    invokeMock.mockClear()
    await mod.scanGatewayLan(9000)
    expect(invokeMock).toHaveBeenLastCalledWith('scan_gateway_lan', { port: 9000 })
  })

  // Web 模式扫描返回空数组（不是 undefined），调用方直接遍历即可
  it('Web 模式 scanGatewayLan 返回空数组', async () => {
    const mod = await loadModule()
    await expect(mod.scanGatewayLan()).resolves.toEqual([])
    expect(invokeMock).not.toHaveBeenCalled()
  })

  // 锁定健康检查契约：Web 模式返回 null（由 health badge 自行轮询），Tauri 走 check_gateway_health
  it('checkGatewayHealth 在 Web 模式返回 null，Tauri 模式调用 check_gateway_health', async () => {
    const mod = await loadModule()
    await expect(mod.checkGatewayHealth()).resolves.toBeNull()

    isTauriMock.mockReturnValue(true)
    const health = { reachable: true, latency_ms: 12, error: null }
    invokeMock.mockResolvedValue(health)
    const mod2 = await loadModule()
    await expect(mod2.checkGatewayHealth()).resolves.toBe(health)
    expect(invokeMock).toHaveBeenCalledWith('check_gateway_health')
  })

  // 锁定探测任意 URL 的契约：invoke check_url_health{url}
  it('checkUrlHealth 调用 check_url_health 并透传 url', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue({ reachable: false, latency_ms: 0, error: 'unreachable' })
    const mod = await loadModule()

    await mod.checkUrlHealth('http://192.168.1.20:8080')
    expect(invokeMock).toHaveBeenCalledWith('check_url_health', { url: 'http://192.168.1.20:8080' })
  })

  // SSRF 防护契约：内网/元数据地址必须在 invoke 之前被前端拦截并抛错，
  // 否则 Tauri 进程会代为发出请求（绕过浏览器同源策略）。
  it('forwardWebhook 拦截内网 URL 且不 invoke', async () => {
    isTauriMock.mockReturnValue(true)
    const mod = await loadModule()

    await expect(mod.forwardWebhook('http://127.0.0.1:8080/hook', 'x')).rejects.toThrow(
      /webhook url blocked/,
    )
    expect(invokeMock).not.toHaveBeenCalled()
  })

  // 合法外网地址：原样透传 url / body 给 Rust 端 forward_webhook
  it('forwardWebhook 合法 URL 调用 forward_webhook 并透传 url 与 body', async () => {
    isTauriMock.mockReturnValue(true)
    invokeMock.mockResolvedValue(undefined)
    const mod = await loadModule()

    await mod.forwardWebhook('https://example.com/hook', 'payload')

    expect(invokeMock).toHaveBeenCalledWith('forward_webhook', {
      url: 'https://example.com/hook',
      body: 'payload',
    })
  })
})
