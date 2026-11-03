import { request } from './index'
import type { ProtocolCapabilities } from './generated/ProtocolCapabilities'

/** 单个协议的描述符（GET /api/protocols 响应元素） */
export interface ProtocolCapabilityEntry {
  name: string
  /** 展示名（后端注册表提供，前端不再硬编码协议清单） */
  label?: string
  /** 链路形态：串行总线 / 网络 / 本机（GPIO） */
  transport?: 'serial' | 'tcp' | 'local'
  /** 新建总线/设备的默认超时（ms） */
  default_timeout_ms?: number
  /** 地址输入提示（表单 placeholder，设备/点位地址） */
  address_hint?: string
  /** 总线对端地址提示（`tcp_addr` 的 placeholder）；纯串行协议为空串 */
  endpoint_hint?: string
  /** 协议变体下拉项；无变体概念时为空数组 */
  variants?: Array<{ value: string; label: string }>
  /** 协议标签色（`el-tag` 的 type），前端不再按协议名硬编码配色 */
  tag_color?: 'primary' | 'success' | 'warning' | 'info' | 'danger'
  capabilities: ProtocolCapabilities
}

/**
 * 协议描述符列表
 *
 * 后端来自南向协议注册表（单一来源），前端据此生成协议下拉、标签、默认超时、
 * 地址/端点提示与变体项，并动态渲染操作入口（如仅对 `supports_write` 的协议显示写入面板）。
 */
export function listProtocolCapabilities(): Promise<ProtocolCapabilityEntry[]> {
  return request<ProtocolCapabilityEntry[]>({ url: '/api/protocols', method: 'GET' })
}

/** 单条总线的诊断结果（GET /api/protocols/:name/diag 响应元素） */
export interface ProtocolDiagEntry {
  /** 总线名 */
  bus: string
  /** 是否真正执行了诊断；false 时 detail 是未执行的原因而非诊断结论 */
  executed: boolean
  /** 诊断输出（SZL 模块信息 / I-Am 设备信息 / GPIO 节点），或未执行的原因 */
  detail: string
}

/** 按协议聚合的诊断结果 */
export interface ProtocolDiagResult {
  protocol: string
  /** 该协议是否有运行实例声明了真实诊断实现（supports_diag） */
  supported: boolean
  entries: ProtocolDiagEntry[]
}

/**
 * 协议诊断（P1）
 *
 * 后端调用 `Collector::diag`（s7comm 读 SZL、bacnet 发 Who-Is、iec104 建链+总召唤、
 * gpio 读芯片路径）。
 * 诊断会占用与采集相同的链路，后端对未暂停的总线会拒绝执行并在 `detail` 说明原因；
 * 需要真实跑诊断时先 `POST /api/buses/:name/pause` 暂停该总线采集。
 */
export function getProtocolDiag(name: string): Promise<ProtocolDiagResult> {
  return request<ProtocolDiagResult>({
    url: `/api/protocols/${encodeURIComponent(name)}/diag`,
    method: 'GET',
  })
}
