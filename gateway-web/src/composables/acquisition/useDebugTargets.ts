/**
 * 调试工具共享目标源：真实设备列表 + 真实总线列表 + 跨页面共享的「当前调试目标」
 *
 * 背景：协议工具各页面（参数探测 / 从站扫描 / 寄存器扫描 / 抓包筛选）
 * 曾使用硬编码 mock 选项（dev_x / dev_1-3 / 静态 BUSES 常量），与网关真实配置
 * 脱节——现场选了不存在的设备/总线只会得到失败，且各页面可选值互相矛盾。
 *
 * 这里统一从真实接口加载：
 * - 设备：GET /api/devices（listDevices）
 * - 总线：GET /api/config 的 buses 数组，bus 标识为 port_name
 *   （与从站扫描、抓包包体的 bus 字段、useDiscoverWizard 一致）
 *
 * 共享调试目标（审查#5）：选型原先各页面各自持有 local ref，从抓包切到扫描
 * 要重选一遍总线/设备。现在 selectedDeviceId / selectedBus 是模块级单例并
 * 持久化到 localStorage——任一页改选，其余调试页跟随；选项加载后若当前
 * 选择已失效（设备被删/配置变更），自动回退到第一个可选项。
 *
 * 加载失败由请求层统一弹窗（GET 全局提示），这里保持静默只暴露空选项。
 */
import { ref, computed, watch, onMounted } from 'vue'
import { listDevices } from '@/api/devices'
import { getConfig } from '@/api/config'
import { scopedGet, scopedRemove, scopedSet } from '@/utils/gatewayStorage'
import type { DeviceConfig, BusConfig } from '@/api/types'

const LS_DEVICE_KEY = 'gateway.debug-target.device-id'
const LS_BUS_KEY = 'gateway.debug-target.bus'

// 模块级单例：跨页面共享的当前调试目标（审查#5）。
// 有意为之的全局状态——这正是「跨调试页共享选型」的语义本体，量比一个 Pinia store
// 小得多（两个字符串 + 持久化），故按 lint 规则指引显式豁免。
// eslint-disable-next-line no-restricted-syntax
const selectedDeviceId = ref(scopedGet(LS_DEVICE_KEY) ?? '')
// eslint-disable-next-line no-restricted-syntax
const selectedBus = ref(scopedGet(LS_BUS_KEY) ?? '')
watch(selectedDeviceId, (v) => {
  if (v) scopedSet(LS_DEVICE_KEY, v)
  else scopedRemove(LS_DEVICE_KEY)
})
watch(selectedBus, (v) => {
  if (v) scopedSet(LS_BUS_KEY, v)
  else scopedRemove(LS_BUS_KEY)
})

/**
 * 只需要共享「当前调试目标」而不需要选项列表的页面（如点位读写/读取性能，
 * 它们自行加载设备详情列表）用本入口，避免多余地触发 devices/config 加载。
 */
export function useSharedDebugTarget() {
  return { selectedDeviceId, selectedBus }
}

export function useDebugTargets() {
  const devices = ref<DeviceConfig[]>([])
  const buses = ref<BusConfig[]>([])
  const loading = ref(false)
  /** 首次加载完成后才允许校正共享选择，避免「尚未加载」被误判为「选择失效」而清掉持久值 */
  const loaded = ref(false)

  /** 设备下拉选项：label 展示 名称 (id)，value 用 id（接口入参） */
  const deviceOptions = computed(() =>
    devices.value.map((d) => ({ label: `${d.name} (${d.id})`, value: d.id })),
  )

  /** 总线下拉选项：value 为 port_name（后端 bus 标识），label 附协议便于现场区分 */
  const busOptions = computed(() =>
    buses.value.map((b) => ({
      label: b.tcp_addr ? `${b.tcp_addr} (${b.protocol})` : `${b.port_name} (${b.protocol})`,
      value: b.port_name,
    })),
  )

  // 选项就绪后校验共享选择：空或已失效时回退到第一个可选项（与原各页「默认选中第一项」行为一致）
  watch([deviceOptions, loaded], ([list, ok]) => {
    if (!ok) return
    if (!list.some((o) => o.value === selectedDeviceId.value)) {
      selectedDeviceId.value = list[0]?.value ?? ''
    }
  }, { immediate: true })
  watch([busOptions, loaded], ([list, ok]) => {
    if (!ok) return
    if (!list.some((o) => o.value === selectedBus.value)) {
      selectedBus.value = list[0]?.value ?? ''
    }
  }, { immediate: true })

  async function load() {
    loading.value = true
    try {
      const [devs, cfg] = await Promise.all([listDevices(), getConfig()])
      devices.value = Array.isArray(devs) ? devs : []
      buses.value = Array.isArray(cfg.buses) ? cfg.buses : []
    } finally {
      loading.value = false
      loaded.value = true
    }
  }

  onMounted(load)

  return { devices, buses, deviceOptions, busOptions, loading, reload: load, selectedDeviceId, selectedBus }
}
