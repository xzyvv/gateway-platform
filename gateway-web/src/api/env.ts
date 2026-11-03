/**
 * 运行环境检测
 *
 * Tauri 桌面端运行时会在 window 上注入 __TAURI_INTERNALS__，
 * Web 端无此全局对象，据此区分 axios 适配器选型。
 */

// Tauri 运行时注入的全局对象类型声明
declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown
  }
}

/** 是否运行在 Tauri 桌面客户端中 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__
}