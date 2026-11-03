/**
 * 协议元数据统一逻辑
 * 协议展示名、标签颜色、下拉选项、默认超时、地址 placeholder
 * 供 Dashboard / Devices / Points 共用，避免重复定义。
 *
 * 数据源：后端 `GET /api/protocols`（南向协议注册表是唯一来源）。
 * 页面登录后调用 {@link loadProtocolMeta} 拉取一次并原地更新下方响应式集合；
 * 拉取失败（离线 / 旧后端）时保留内置兜底清单，不阻断调试。
 */

import { reactive } from 'vue'

import { listProtocolCapabilities, type ProtocolCapabilityEntry } from '@/api/protocols'

export type ProtocolTagType = 'primary' | 'success' | 'warning' | 'info' | 'danger'

/** 协议下拉项 */
export interface ProtocolOption {
  label: string
  value: string
}

/** 离线兜底：旧版前端仅支持这 4 个串行/远动协议，保证断网时仍可配置 */
const FALLBACK_OPTIONS: ProtocolOption[] = [
  { label: 'Modbus', value: 'modbus' },
  { label: 'DLT645', value: 'dlt645' },
  { label: 'CJT188', value: 'cjt188' },
  { label: 'IEC104', value: 'iec104' },
]

const FALLBACK_LABELS: Record<string, string> = {
  modbus: 'Modbus',
  dlt645: 'DLT645',
  cjt188: 'CJT188',
  iec104: 'IEC104',
}

/**
 * 离线兜底的默认超时（ms），仅在 `/api/protocols` 拉取失败时使用。
 *
 * 串行表计总线响应快，1s 足够；TCP 类协议单次应答/总召唤窗口更长，取 3s。
 */
const FALLBACK_TIMEOUT_MS: Record<string, number> = {
  modbus: 1000,
  dlt645: 1000,
  cjt188: 1000,
  iec104: 3000,
  iec101: 3000,
  fins: 3000,
  mc: 3000,
  s7comm: 3000,
  bacnet: 3000,
  opcua: 3000,
  bacnet_mstp: 1000,
}

const FALLBACK_ADDRESS_HINTS: Record<string, string> = {
  modbus: '如: 1（从站号 1-247）',
  dlt645: '如: 1（表号，1-65535）',
  cjt188: '如: 1（表号，1-99999999）',
  iec104: '如: 1（信息体地址 IA）',
}

/**
 * 离线兜底的总线对端地址提示（`tcp_addr`）。
 *
 * 仅网络类协议（以及可切 TCP 的 modbus）需要；纯串行协议不在表内，取空串。
 */
const FALLBACK_ENDPOINT_HINTS: Record<string, string> = {
  modbus: '如: 192.168.1.10:502（可选，填写后走 Modbus TCP）',
  iec104: '如: 192.168.1.10:2404',
  fins: '如: 192.168.1.10:9600（UDP 可追加 ?node=NN）',
  mc: '如: 192.168.1.10:5000',
  s7comm: '如: 192.168.1.10:102?rack=0&slot=2',
  bacnet: '如: 192.168.1.100:47808',
  opcua: '如: opc.tcp://192.168.1.10:4840',
  bacnet_mstp: '可选: master=0&max_master=127',
}

/**
 * 离线兜底的协议变体下拉项（仅在 `/api/protocols` 拉取失败时使用）。
 *
 * 变体的取值集合由后端注册表定义，这里只为兜底清单里的协议保持可配置。
 */
const FALLBACK_VARIANTS: Record<string, ProtocolOption[]> = {
  modbus: [
    { label: 'RTU', value: 'rtu' },
    { label: 'ASCII', value: 'ascii' },
  ],
  dlt645: [
    { label: '2007（国标默认）', value: '2007' },
    { label: '1997（老款表）', value: '1997' },
  ],
  cjt188: [
    { label: '2018（内建方言）', value: '2018' },
    { label: '2004 标准方言', value: '2004' },
  ],
}

/**
 * 离线兜底的链路形态。串行协议只支持串口参数；TCP 协议只用「对端地址」，
 * 不再显示波特率/校验位等无效字段。
 */
const FALLBACK_TRANSPORT: Record<string, 'serial' | 'tcp' | 'local'> = {
  modbus: 'serial',
  dlt645: 'serial',
  cjt188: 'serial',
  iec101: 'serial',
  bacnet_mstp: 'serial',
  iec104: 'tcp',
  fins: 'tcp',
  mc: 'tcp',
  s7comm: 'tcp',
  bacnet: 'tcp',
  opcua: 'tcp',
  gpio: 'local',
}

/** 协议列表（用于 el-select），由后端描述符原地刷新 */
export const protocolOptions = reactive<ProtocolOption[]>(FALLBACK_OPTIONS.map(o => ({ ...o })))

/** 协议展示名映射 */
export const protocolLabel = reactive<Record<string, string>>({ ...FALLBACK_LABELS })

/** 协议默认超时（ms） */
// eslint-disable-next-line no-restricted-syntax -- 有意共享的后端协议元数据缓存，由 applyProtocolMetadata 原地刷新
const protocolTimeoutMs = reactive<Record<string, number>>({ ...FALLBACK_TIMEOUT_MS })

/** 协议地址输入提示 */
// eslint-disable-next-line no-restricted-syntax -- 同上：跨页面共享的协议元数据，原地刷新保持引用
const protocolAddressHint = reactive<Record<string, string>>({ ...FALLBACK_ADDRESS_HINTS })

/**
 * 协议标签色（`el-tag` 的 type）。
 *
 * P5：与 label / transport / 默认超时 / 地址提示同源于后端注册表
 * （`GET /api/protocols` 的 `tag_color`），前端不再按协议名硬编码配色，
 * 新增协议无需改动前端色表。
 */
// eslint-disable-next-line no-restricted-syntax -- 同上：后端注册表下发的标签色，跨页面共享且原地刷新
const protocolTagColor = reactive<Record<string, ProtocolTagType>>({})

/** 协议链路形态（决定总线表单显示串口参数还是对端地址） */
// eslint-disable-next-line no-restricted-syntax -- 同上：后端注册表下发，原地刷新
const protocolTransport = reactive<Record<string, 'serial' | 'tcp' | 'local'>>({
  ...FALLBACK_TRANSPORT,
})

/** 总线对端地址（`tcp_addr`）输入提示 */
// eslint-disable-next-line no-restricted-syntax -- 同上：后端注册表下发，原地刷新
const protocolEndpointHint = reactive<Record<string, string>>({ ...FALLBACK_ENDPOINT_HINTS })

/** 协议变体下拉项；无变体的协议不在表中（取空数组） */
// eslint-disable-next-line no-restricted-syntax -- 同上：后端注册表下发，原地刷新
const protocolVariants = reactive<Record<string, ProtocolOption[]>>(
  Object.fromEntries(
    Object.entries(FALLBACK_VARIANTS).map(([k, v]) => [k, v.map(o => ({ ...o }))]),
  ),
)

/** 点位级协议：可出现在点位 protocol 字段，但不作为设备/总线协议选项 */
const POINT_LEVEL_PROTOCOLS = new Set(['gpio'])

/** 未登记协议的兜底默认超时（与串行总线默认一致） */
export const DEFAULT_PROTOCOL_TIMEOUT_MS = 1000

/** 未登记协议的兜底标签色（Element Plus 中性色） */
export const DEFAULT_TAG_COLOR: ProtocolTagType = 'info'

/**
 * 用后端协议描述符刷新本地元数据（原地更新，保持响应式引用不变）。
 *
 * 过滤点位级协议（gpio）后重建下拉项；空列表（旧后端/解析异常）时保持现状。
 */
export function applyProtocolMetadata(entries: ProtocolCapabilityEntry[]): void {
  if (!Array.isArray(entries) || entries.length === 0) return
  const options: ProtocolOption[] = []
  for (const entry of entries) {
    if (!entry?.name) continue
    const label = entry.label?.trim() || protocolLabel[entry.name] || entry.name.toUpperCase()
    protocolLabel[entry.name] = label
    if (entry.default_timeout_ms && entry.default_timeout_ms > 0) {
      protocolTimeoutMs[entry.name] = entry.default_timeout_ms
    }
    if (entry.address_hint) {
      protocolAddressHint[entry.name] = entry.address_hint
    }
    if (entry.endpoint_hint) {
      protocolEndpointHint[entry.name] = entry.endpoint_hint
    }
    if (entry.transport) {
      protocolTransport[entry.name] = entry.transport
    }
    if (Array.isArray(entry.variants)) {
      protocolVariants[entry.name] = entry.variants
        .filter(v => v?.value)
        .map(v => ({ value: v.value, label: v.label || v.value }))
    }
    if (entry.tag_color) {
      protocolTagColor[entry.name] = entry.tag_color
    }
    if (!POINT_LEVEL_PROTOCOLS.has(entry.name)) {
      options.push({ label, value: entry.name })
    }
  }
  if (options.length === 0) return
  protocolOptions.splice(0, protocolOptions.length, ...options)
}

let metaLoaded = false
let metaInFlight: Promise<void> | null = null

/**
 * 拉取协议元数据（进程级只拉一次，可 force 强制刷新）。
 *
 * 失败静默：保留兜底清单，避免离线调试时弹错。
 */
export async function loadProtocolMeta(force = false): Promise<void> {
  if (metaInFlight) return metaInFlight
  if (metaLoaded && !force) return
  metaInFlight = listProtocolCapabilities()
    .then(applyProtocolMetadata)
    .catch(() => { /* 离线/旧后端：保留内置兜底 */ })
    .finally(() => {
      metaLoaded = true
      metaInFlight = null
    })
  return metaInFlight
}

/**
 * 协议对应的标签颜色。
 *
 * 优先取后端注册表下发的 `tag_color`；未登记（旧后端 / 元数据未加载）回落到
 * `DEFAULT_TAG_COLOR`，不再维护按协议名的前端色表。
 */
export function protocolType(p: string): ProtocolTagType {
  return protocolTagColor[p] ?? DEFAULT_TAG_COLOR
}

/** 根据协议生成地址输入框 placeholder（优先使用后端描述符） */
export function protocolAddressPlaceholder(protocol: string): string {
  return protocolAddressHint[protocol] ?? '设备地址'
}

/** 取某协议的链路形态（`serial` / `tcp` / `local`），未登记按串行处理 */
export function protocolTransportOf(protocol: string): 'serial' | 'tcp' | 'local' {
  return protocolTransport[protocol] ?? 'serial'
}

/**
 * 总线对端地址（`tcp_addr`）输入提示。
 *
 * 纯串行协议（dlt645 / cjt188 / iec101）没有端点概念，返回通用 `host:port` 提示，
 * 由调用方按 {@link protocolTransportOf} 决定是否展示该字段。
 */
export function protocolEndpointPlaceholder(protocol: string): string {
  return protocolEndpointHint[protocol] ?? '如: 192.168.1.10:502'
}

/** 取某协议的变体下拉项；无变体概念时返回空数组 */
export function protocolVariantsOf(protocol: string): ProtocolOption[] {
  return protocolVariants[protocol] ?? []
}

/** 取某协议新建总线的默认超时（后端描述符优先，未登记回落到 {@link DEFAULT_PROTOCOL_TIMEOUT_MS}） */
export function protocolDefaultTimeoutMs(protocol: string): number {
  return protocolTimeoutMs[protocol] ?? DEFAULT_PROTOCOL_TIMEOUT_MS
}

/**
 * 判断给定超时值是否等于「某个协议的默认值」。
 *
 * 用于协议切换时决定是否跟随更新：仅当当前值仍是默认值（用户未在
 * 通信参数页自定义过）才改写，避免覆盖用户显式设置。
 */
export function isProtocolDefaultTimeout(ms: number): boolean {
  return Object.values(protocolTimeoutMs).includes(ms)
}
