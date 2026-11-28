/**
 * 网关运行时上下文的只读入口（DD-86）。
 *
 * `utils/gatewayStorage` 需要激活网关名做 localStorage 命名空间隔离，
 * `utils/websocket/endpoint` 需要网关 baseUrl 拼 WS 地址——二者都在 utils 层，
 * 不能反向依赖 api。改为 api/gatewayConfig 在模块加载时注册提供者，utils 只读。
 *
 * 未注册（Web 模式 / 模块尚未加载）时返回空值，与改造前「未配置网关」行为一致。
 */

interface GatewayContextProvider {
  /** 当前激活网关的基础 URL；Web 模式返回空串 */
  baseUrl: () => string
  /** 当前激活网关名；未配置 / Web 模式返回 null */
  activeName: () => string | null
}

let provider: GatewayContextProvider | null = null

/** 由 api/gatewayConfig 在模块初始化时调用 */
export function registerGatewayContext(p: GatewayContextProvider): void {
  provider = p
}

/** 读取网关 baseUrl（无提供者或抛错时返回空串） */
export function getGatewayBaseUrlSafe(): string {
  try {
    return provider?.baseUrl() ?? ''
  } catch {
    return ''
  }
}

/** 读取激活网关名（无提供者或抛错时返回 null） */
export function getActiveGatewayNameSafe(): string | null {
  try {
    return provider?.activeName() ?? null
  } catch {
    return null
  }
}
