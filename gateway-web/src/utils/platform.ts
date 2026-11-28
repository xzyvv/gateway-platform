/**
 * 运行环境检测（下沉到 utils，DD-86）。
 *
 * api 与 utils 都要判断是否处于 Tauri 运行时；放在 api 层会让 utils 反向依赖 api，
 * 现由 utils 拥有实现，api/env.ts 仅做再导出。
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
