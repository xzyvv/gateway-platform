import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { getConfig } from '@/api/config'
import { fetchPointQualities } from '@/composables/data/usePointQuality'
import { extractErrorMessage } from '@/utils/errorHandler'
import type { PointQuality } from '@/api/dataQuality'
import type { PointConfig } from '@/api/types'
import type { PointsState } from './types'
import { FALLBACK_NORTH_INTERVAL } from './validation'

/** 设备列表加载失败时的兜底文案 */
const DEVICE_LOAD_ERROR = '无法获取设备列表'

/**
 * 点位清单的数据加载
 *
 * 负责设备列表、点位列表的拉取，以及多选变更；
 * 点位请求带序号守卫，避免快速切换设备时旧响应覆盖新设备的清单。
 */
export function createPointQuery(state: PointsState) {
  const { devices, points, runtimePoints, loading, loadError, selectedDevice, selection, northInterval } = state

  /**
   * 读取北向上报周期：点位采集周期必须小于它（后端 validate_cycle_conflict）。
   *
   * 失败或 MQTT 未启用时保持兜底值 60s——该值只用于表单提示与新建点位的收敛，
   * 读不到不应该阻塞设备/点位列表。
   */
  async function loadNorthInterval() {
    try {
      const cfg = await getConfig()
      if (state.isUnmounted()) return
      northInterval.value =
        cfg.mqtt?.enabled && cfg.mqtt.publish_interval > 0
          ? cfg.mqtt.publish_interval
          : FALLBACK_NORTH_INTERVAL
    } catch {
      // 无配置读取权限或网关暂时不可达：沿用兜底值
    }
  }

  // 多选
  function onSelectionChange(rows: PointConfig[]) {
    selection.value = rows
  }

  async function loadDevices() {
    // 北向周期与设备列表并行拉取：慢/失败都不阻塞列表
    void loadNorthInterval()
    try {
      // 运行时点位快照（含模板展开）与设备列表并行；快照失败不阻塞，只少模板行
      const [data, rtPts] = await Promise.all([
        listDevices(),
        fetchPointQualities().catch(() => [] as PointQuality[]),
      ])
      if (state.isUnmounted()) return
      devices.value = data
      runtimePoints.value = rtPts
      if (loadError.value === DEVICE_LOAD_ERROR) loadError.value = ''
      // 进页默认选中第一台设备：否则要先手动点下拉才有清单，新增/导入按钮一直禁用
      const first = devices.value[0]
      if (!selectedDevice.value && first) {
        selectedDevice.value = first.id
      }
      if (selectedDevice.value) void loadPoints()
    } catch (e) {
      // 设备下拉加载失败会让整页看起来「没有设备」，必须显式说明
      if (!state.isUnmounted()) loadError.value = extractErrorMessage(e, DEVICE_LOAD_ERROR)
    }
  }

  /**
   * 点位请求序号。
   *
   * 修复（切换设备竞态）：原实现 `await listPoints(...)` 之后直接写 `points.value`，
   * 只有 `unmounted` 守卫。快速把设备从 A 切到 B 时，若 A 的响应后到，就会用 A 的点位
   * 覆盖 B 的列表——表格显示 A 的点位而下拉框是 B，后续删除/复制等操作会作用在错误
   * 设备上（复制对话框以 points 为源、selectedDevice 为目标，可直接写错设备）。
   * 与 useSystem / useDashboardData 同用序号守卫：只接受最新一次请求的结果。
   */
  let loadSeq = 0

  async function loadPoints() {
    const seq = ++loadSeq
    selection.value = []
    if (!selectedDevice.value) {
      points.value = []
      return
    }
    loading.value = true
    loadError.value = ''
    try {
      const data = await listPoints(selectedDevice.value)
      if (state.isUnmounted() || seq !== loadSeq) return
      points.value = data
    } catch (e) {
      // 失败时不能只留空表：会被误读为「该设备没有点位」
      if (!state.isUnmounted() && seq === loadSeq) loadError.value = extractErrorMessage(e, '无法获取点位列表')
    } finally {
      // 旧请求不得清掉新请求的 loading
      if (!state.isUnmounted() && seq === loadSeq) loading.value = false
    }
  }

  return { onSelectionChange, loadDevices, loadPoints }
}
