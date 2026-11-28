// === 网络状态监听（断网提示） ===
// 模块加载时注册 window online/offline 事件，断网/恢复时发通知事件。
// ③ api → stores 事件化：不在 api 层直接取通知 store，由 stores/httpEvents 落库。
import { emitHttpNotify } from './uiEvents'

let networkOnline = typeof navigator !== 'undefined' ? navigator.onLine : true

if (typeof window !== 'undefined') {
  window.addEventListener('online', () => {
    if (!networkOnline) {
      networkOnline = true
      emitHttpNotify({ level: 'success', message: '网络已恢复' })
    }
  })
  window.addEventListener('offline', () => {
    if (networkOnline) {
      networkOnline = false
      emitHttpNotify({ level: 'error', message: '网络已断开，请检查连接' })
    }
  })
}
