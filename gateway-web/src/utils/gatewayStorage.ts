/**
 * 多网关 localStorage 命名空间隔离（#6）
 *
 * Tauri 桌面端可在多个网关间切换，但 webview 的 localStorage 由所有网关共享：
 * 若按固定 key 存储（收藏、转换规则、限流规则、数据质量规则、分层策略等），
 * 切换网关后 A 网关的配置会出现在 B 网关里，表现为「配置串台 / 规则莫名变化」，
 * 且切网关只 reload 不清除，问题会一直存在。
 *
 * 这里按激活网关名给 key 加前缀，实现天然隔离。Web 模式（非 Tauri / 未配置网关）
 * 前缀为空，等价于原始 key，行为与改造前完全一致。
 */
import { getActiveGatewayName } from '@/api/gatewayConfig'

const SEP = '::'

/** 当前命名空间前缀；无激活网关时为空串（等价于不隔离） */
export function gatewayNamespace(): string {
  try {
    const name = getActiveGatewayName()
    return name ? `gw:${name}${SEP}` : ''
  } catch {
    return ''
  }
}

/** 计算隔离后的真实 key */
export function scopedKey(base: string): string {
  return `${gatewayNamespace()}${base}`
}

/**
 * 读取（带旧数据迁移）。
 *
 * 首次升级到隔离 key 时，若旧的无前缀 key 仍有数据则迁移到当前网关命名空间，
 * 避免老用户升级后配置「丢失」。
 */
export function scopedGet(base: string): string | null {
  try {
    const key = scopedKey(base)
    const scoped = localStorage.getItem(key)
    if (scoped !== null) return scoped
    if (key !== base) {
      const legacy = localStorage.getItem(base)
      if (legacy !== null) {
        localStorage.setItem(key, legacy)
        return legacy
      }
    }
    return null
  } catch {
    return null
  }
}

/** 写入当前网关命名空间 */
export function scopedSet(base: string, value: string): void {
  localStorage.setItem(scopedKey(base), value)
}

/** 删除当前网关命名空间下的项 */
export function scopedRemove(base: string): void {
  localStorage.removeItem(scopedKey(base))
}

/**
 * ===== 全局（不随网关隔离）的访问入口 =====
 *
 * 主题、列宽、首页布局、Hub 折叠状态这类**纯用户界面偏好**属于「这台浏览器上的人」，
 * 而不是「这台网关的配置」：切到另一台网关时主题突然变亮、列宽被重置反而是 bug。
 *
 * 但仍然要求走本模块而不是在业务代码里直接 `localStorage.xxx`：
 * 全仓只有本模块（与 secureStorage 这个 token 专用存储层）允许直接触达 localStorage，
 * 未来若要换成 IndexedDB / Tauri store，只改这里即可。
 */
export function globalGet(base: string): string | null {
  try {
    return localStorage.getItem(base)
  } catch {
    return null
  }
}

/** 写全局 UI 偏好（不隔离） */
export function globalSet(base: string, value: string): void {
  localStorage.setItem(base, value)
}

/** 删全局 UI 偏好（不隔离） */
export function globalRemove(base: string): void {
  localStorage.removeItem(base)
}
