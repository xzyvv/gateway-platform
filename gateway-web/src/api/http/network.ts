// === 网络状态监听（断网提示） ===
// 模块加载时注册 window online/offline 事件，断网/恢复时弹通知。
// useNotificationsStore 延迟到事件触发时调用（避免 pinia 未初始化）。
import { useNotificationsStore } from '../../stores/notifications'

let networkOnline = typeof navigator !== 'undefined' ? navigator.onLine : true

if (typeof window !== 'undefined') {
  window.addEventListener('online', () => {
    if (!networkOnline) {
      networkOnline = true
      try {
        useNotificationsStore().success('网络已恢复')
      } catch { /* pinia 未激活时静默 */ }
    }
  })
  window.addEventListener('offline', () => {
    if (networkOnline) {
      networkOnline = false
      try {
        useNotificationsStore().error('网络已断开，请检查连接')
      } catch { /* pinia 未激活时静默 */ }
    }
  })
}
