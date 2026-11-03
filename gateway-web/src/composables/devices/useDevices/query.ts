/**
 * 设备列表加载与路由高亮定位
 *
 * 负责「设备列表」这一块：拉取列表、维护加载态与失败原因、
 * 以及从「设备模板 / 点位」页跳转过来时的高亮定位。
 */
import { computed } from 'vue'
import { listDevices } from '@/api/devices'
import { extractErrorMessage } from '@/utils/errorHandler'
import type { DeviceConfig } from '@/api/types'
import type { DeviceState } from './types'

/**
 * 设备列表查询
 *
 * @param state 共享状态（devices / loading / loadError / route / 卸载守卫）
 */
export function createDeviceQuery(state: DeviceState) {
  const highlightId = computed(() => {
    return (state.route.query.id as string) || (state.route.query.highlight as string) || ''
  })

  function rowClass({ row }: { row: DeviceConfig }) {
    return highlightId.value && row.id === highlightId.value ? 'is-highlighted' : ''
  }

  /** 滚动到高亮行并闪烁提示（列表渲染完再滚，否则目标行还不存在） */
  function scrollToHighlightedRow() {
    setTimeout(() => {
      const row = document.querySelector('.device-table .el-table__row.is-highlighted') as HTMLElement | null
      if (row) {
        row.scrollIntoView({ behavior: 'smooth', block: 'center' })
        row.classList.add('is-highlighted-flash')
        setTimeout(() => row.classList.remove('is-highlighted-flash'), 3000)
      }
    }, 100)
  }

  async function loadDevices() {
    state.loading.value = true
    state.loadError.value = ''
    try {
      const data = await listDevices()
      if (state.isUnmounted()) return
      state.devices.value = data
    } catch (e) {
      // 拦截器对 GET 会弹一次 toast，但页面本身仍需要显式失败态：
      // 静默吞掉会让空列表看起来像「设备已被删除」
      if (!state.isUnmounted()) state.loadError.value = extractErrorMessage(e, '无法获取设备列表')
    } finally {
      if (!state.isUnmounted()) state.loading.value = false
      if (highlightId.value) scrollToHighlightedRow()
    }
  }

  return { highlightId, rowClass, loadDevices }
}
