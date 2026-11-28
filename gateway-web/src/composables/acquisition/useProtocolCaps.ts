/**
 * 协议能力门控（共享）：`GET /api/protocols` → 协议名 → 能力位映射
 *
 * 调试工具页（从站扫描 / 寄存器扫描 / 参数探测）都按能力位决定按钮可用性，
 * 而不是按协议名硬编码，后端注册表新增能力即为前端新增入口。
 *
 * 能力元数据缺失（加载失败）时一律按「不支持」处理：能力未知不等于具备，
 * 否则用户会看到一个「点了必然失败」的按钮，误判为设备/网关故障。
 *
 * ## 为什么是模块级单例
 *
 * 协议注册表在一个网关进程的生命周期内是静态的——它由后端编译期注册的协议
 * 决定，运行中不会变；切换网关会 `location.reload()` 整页重载，单例自然重建。
 * 而此前 `onMounted(load)` 建在本文件里、状态建在 `useProtocolCaps()` 内部，
 * 于是**每个调用方各持一份快照、各打一轮 `GET /api/protocols`**：
 * 调试台的从站扫描（useRegisterScan）+ 参数探测（useAutoDetect）
 * + 点位读写页的能力徽标（useProtocolGates）共 3 条请求，拿到的还是同一份数据。
 *
 * 现在 `caps / loaded / failed` 提到模块级，请求用 `inflight` 单飞：
 * - 同一次渲染里的多处调用合并成**一次**请求；
 * - 后挂载的页面直接复用已加载的快照，不再空等一轮网络往返；
 * - 加载失败时 `loaded` 仍置 true（表示「这一轮已结束」），但 `failed` 为 true，
 *   下一次挂载会**自动重试**——否则用户修好网络后换个调试页仍旧全是灰按钮。
 */
import { onMounted, ref } from 'vue'
import { listProtocolCapabilities } from '@/api/acquisition/protocols'
import type { ProtocolCapabilities } from '@/api/generated/ProtocolCapabilities'

// 模块级单例：一份网关协议能力快照（理由见文件头）。
// 有意为之的全局状态——协议注册表在一个网关进程内静态不变，
// 每个页面各持一份正是本节要消除的重复请求来源，故按 lint 规则指引显式豁免。
// eslint-disable-next-line no-restricted-syntax
const caps = ref<Record<string, ProtocolCapabilities>>({})
/** 加载流程已结束（成功或失败），用于区分「加载中」与「确实不支持」 */
// eslint-disable-next-line no-restricted-syntax
const loaded = ref(false)
/** 最近一轮加载是否失败。只表示「这次没拿到」，不代表能力为空，二者不可混用 */
// eslint-disable-next-line no-restricted-syntax
const failed = ref(false)
/** 在飞的加载请求：并发挂载共享同一次，避免每个实例各打一轮接口 */
let inflight: Promise<void> | null = null

/**
 * 拉取协议能力（单飞）。
 *
 * 失败不抛：调用方多为 `onMounted`，抛出去只会变成 unhandled rejection；
 * 失败信号通过 `failed` 暴露给 UI。
 */
async function load(): Promise<void> {
  if (inflight) return inflight
  inflight = (async () => {
    try {
      const list = await listProtocolCapabilities()
      const map: Record<string, ProtocolCapabilities> = {}
      for (const e of list) map[e.name] = e.capabilities
      caps.value = map
      // 重试成功后必须复位：否则 caps 已填充但 failed 仍为 true，
      // 调用方会同时看到「有数据」和「加载失败」两种互相矛盾的信号。
      failed.value = false
    } catch {
      failed.value = true
    } finally {
      loaded.value = true
      inflight = null
    }
  })()
  return inflight
}

/**
 * 清空模块级快照（回到「从未加载」状态，下一次挂载会重新请求）。
 *
 * 主要用途是测试隔离：单例状态会在同一个测试文件的用例之间残留，
 * 前一条用例灌进去的能力位会被后一条误认为「已加载」。
 * 生产路径不需要调用——切换网关走整页 `location.reload()`，单例自然重建；
 * 若将来协议注册表支持热更新，也可由这里触发重新加载。
 */
export function resetProtocolCapsCache(): void {
  caps.value = {}
  loaded.value = false
  failed.value = false
  inflight = null
}

export function useProtocolCaps() {
  // 已成功加载过就不再打接口；上次失败则重试（用户可能刚修好网络）。
  // 此处必须在 setup 内注册：onMounted 只能在组件生命周期里调用。
  onMounted(() => {
    if (!loaded.value || failed.value) void load()
  })

  /** 取某协议的能力声明；未知协议 / 元数据未加载时返回 null（按不支持处理） */
  function capsOf(protocol: string): ProtocolCapabilities | null {
    return protocol ? caps.value[protocol] ?? null : null
  }

  return { caps, loaded, failed, load, capsOf }
}
