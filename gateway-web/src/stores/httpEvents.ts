/**
 * HTTP 事件 → store 的接线（③ api → stores 事件化）。
 *
 * api/http 只发事件，本模块在应用启动（app/main.ts）与测试 setup 时加载，
 * 把事件落到通知中心 / 请求日志两个 store，从而消除 `api -> stores` 静态倒挂。
 */
import { setHttpEventsHandlers } from '@/api/http/uiEvents'
import { useNotificationsStore } from './notifications'
import { useRequestLogStore } from './requestLog'

setHttpEventsHandlers({
  notify(e) {
    const store = useNotificationsStore()
    switch (e.level) {
      case 'success':
        store.success(e.message, e.opts)
        break
      case 'warning':
        store.warning(e.message, e.opts)
        break
      case 'info':
        store.info(e.message, e.opts)
        break
      default:
        store.error(e.message, e.opts)
    }
  },
  logRequest(entry) {
    useRequestLogStore().pushLog(entry)
  },
})
