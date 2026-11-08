import { computed, onUnmounted, ref } from 'vue'
import type { DeviceConfig, PointConfig } from '@/api/types'
import type { PointQuality } from '@/api/dataQuality'
import type { PointRow, PointsState } from './types'
import { FALLBACK_NORTH_INTERVAL } from './validation'

/**
 * 点位页共享状态
 *
 * 设备列表、点位列表、多选、加载态、加载错误与派生清单（mergedPoints）在此集中创建，
 * 由各职责模块（query / crud / values / form / batchImport）共享注入，
 * 避免每个模块各自维护一份状态导致不一致。
 */
export function createPointsState(): PointsState {
  const devices = ref<DeviceConfig[]>([])
  const points = ref<PointConfig[]>([])
  /** 运行时点位快照（含模板展开）：进页随设备列表一起拉，失败不阻塞配置点位展示 */
  const runtimePoints = ref<PointQuality[]>([])
  const loading = ref(false)
  /** 加载失败原因；空串表示无错误。失败时不显示空表，避免误判「设备/点位被删」 */
  const loadError = ref('')
  const selectedDevice = ref('')
  // 多选
  const selection = ref<PointConfig[]>([])
  /** 北向上报周期（秒）：进页后由 query.loadDevices 从配置中心刷新 */
  const northInterval = ref(FALLBACK_NORTH_INTERVAL)

  // dialog visibility helpers
  const showCopy = ref(false)
  const showBatchEdit = ref(false)
  const showGen = ref(false)

  /** 组件卸载标记：卸载后不再写状态、不再弹提示 */
  let unmounted = false
  onUnmounted(() => { unmounted = true })

  const currentDeviceName = computed(() => {
    return devices.value.find((d) => d.id === selectedDevice.value)?.name || '—'
  })

  /**
   * 展示用点位 = 配置点位 + 该设备下模板展开的运行时点位（去重）。
   * 模板行补最小 PointConfig 字段（总线/协议继承设备），_source 标记来源；
   * 表格对 _source='template' 的行禁用编辑/删除/勾选。
   */
  const mergedPoints = computed<PointRow[]>(() => {
    const rows: PointRow[] = [...points.value]
    const configured = new Set(points.value.map((p) => p.id))
    const dev = devices.value.find((d) => d.id === selectedDevice.value)
    for (const rp of runtimePoints.value) {
      if (!selectedDevice.value || rp.device_id !== selectedDevice.value) continue
      if (configured.has(rp.point_id)) continue
      rows.push({
        id: rp.point_id,
        name: rp.point_name,
        device_id: rp.device_id,
        bus: dev?.bus ?? '',
        protocol: dev?.protocol ?? '',
        address: '',
        slave_id: null,
        address_num: null,
        data_id: null,
        function_code: null,
        data_type: null,
        byte_order: null,
        register_count: null,
        interval: 1,
        scale: 1,
        offset: 0,
        formula: null,
        ct_ratio: null,
        pt_ratio: null,
        deadband: 0,
        on_change: false,
        unit: null,
        alarm_high: null,
        alarm_low: null,
        aggregation: null,
        modbus_register: null,
        writable: false,
        _source: 'template',
      })
    }
    return rows
  })

  return {
    devices,
    points,
    runtimePoints,
    loading,
    loadError,
    selectedDevice,
    selection,
    northInterval,
    currentDeviceName,
    mergedPoints,
    showCopy,
    showBatchEdit,
    showGen,
    isUnmounted: () => unmounted,
  }
}
