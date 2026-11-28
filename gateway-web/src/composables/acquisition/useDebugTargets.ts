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
 * 选项列表（devices / buses / loading / loaded）同样是**模块级单例**：
 * 原先它们建在 `useDebugTargets()` 内部、且 `onMounted(load)` 也写在 composable 里，
 * 于是「谁调了一次就自己打一轮接口」。而一个页面上常有两处调用（Hub 的
 * ConsoleTargetBar + 页面本身），每进一个 tab 就是 2 轮 GET /api/devices +
 * GET /api/config。改为单例 + 短 TTL 后：
 * - 同一次渲染里的多处调用合并成**一次**请求（`inflight` 单飞）；
 * - TTL（5s）内跨组件复用同一份选项，超时后下一次挂载重新拉取，
 *   设备/总线配置变更不会被长期遮住；
 * - 需要绝对最新配置的场合（如调试向导的「总线检查」）显式 `reload()` 强制刷新。
 *
 * 加载失败由请求层统一弹窗（GET 全局提示），这里保持静默只暴露空选项；
 * 失败时 `loaded` 保持 false，因此下一次挂载会自动重试，且不会把已持久化的
 * 选择误判为「失效」而清空。
 */
import { ref, computed, watch, onMounted } from 'vue'
import { listDevices } from '@/api/acquisition/devices'
import { getConfig } from '@/api/uplink/config'
import { scopedGet, scopedRemove, scopedSet } from '@/utils/storage/gatewayStorage'
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

// 模块级单例：选项列表与加载态（理由见文件头注释）。
// 与上面的共享选择同样是**有意为之**的全局状态——它代表「一份网关配置快照」，
// 每个 tab 各持一份正是本节要消除的重复请求来源，故按 lint 规则指引显式豁免。
// eslint-disable-next-line no-restricted-syntax
const devices = ref<DeviceConfig[]>([])
// eslint-disable-next-line no-restricted-syntax
const buses = ref<BusConfig[]>([])
// eslint-disable-next-line no-restricted-syntax
const loading = ref(false)
/** 首次**成功**加载完成。失败时保持 false，下一次挂载会重试。 */
// eslint-disable-next-line no-restricted-syntax
const loaded = ref(false)
/** 在飞的加载请求：并发挂载共享同一次，避免每个实例各打一轮接口 */
let inflight: Promise<void> | null = null
/**
 * 选项快照的有效期。
 *
 * 旧行为是「每次页面挂载都拉一遍」，配置改动能立刻看到；纯单例会让它变成
 * 「装一次就永远用旧快照」，新建设备后进扫描页看不到新设备。
 * TTL 取 5s：同一次渲染里两处调用（工具栏 + 页面）必然落在窗口内，合并成一轮；
 * 隔几秒再进另一个 tab 则会重新拉取，配置变更不会被长期遮住。
 * 需要绝对最新的场合（调试向导的总线检查）走 `reload()` 强制刷新。
 */
const TARGETS_TTL_MS = 5_000
/** 上次成功加载的时刻（毫秒时间戳；0 表示从未加载成功） */
let loadedAt = 0

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

// 选项就绪后校验共享选择：空或已失效时回退到第一个可选项（与原各页「默认选中第一项」行为一致）。
// 建在模块级：单例状态只需一套校正逻辑，不必每个调用方各建一对 watcher。
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

/**
 * 拉取设备与总线配置（单飞）。
 *
 * - 已有请求在跑：直接复用，不叠加第二轮；
 * - 失败：静默（请求层已统一提示），`loaded` 不复位为 true，选项保持原值。
 */
async function fetchTargets(): Promise<void> {
  if (inflight) return inflight
  loading.value = true
  inflight = (async () => {
    try {
      const [devs, cfg] = await Promise.all([listDevices(), getConfig()])
      devices.value = Array.isArray(devs) ? devs : []
      buses.value = Array.isArray(cfg.buses) ? cfg.buses : []
      loaded.value = true
      loadedAt = Date.now()
    } catch {
      // 失败不抛：调用方多为 onMounted，抛出去只会变成 unhandled rejection
    } finally {
      loading.value = false
      inflight = null
    }
  })()
  return inflight
}

export function useDebugTargets() {
  // 快照仍在有效期内就直接复用；否则（首次 / 过期 / 上次失败）重新拉取。
  // 此处仍需在 setup 内注册：onMounted 必须在组件生命周期内调用。
  onMounted(() => {
    if (!loaded.value || Date.now() - loadedAt > TARGETS_TTL_MS) void fetchTargets()
  })

  return {
    devices,
    buses,
    deviceOptions,
    busOptions,
    loading,
    /** 强制重新拉取（总线/设备可能已变更）；并发调用仍合并成一次请求 */
    reload: fetchTargets,
    selectedDeviceId,
    selectedBus,
  }
}
