// 上行模板配置 REST 客户端
// 提供：模板 CRUD + 编译预览 + 测试发送

import { request } from './index'

export interface UplinkConfig {
  /** 是否启用自定义上行模板 */
  enabled: boolean
  /** JSON 模板字符串（含 ${var} 占位符） */
  template: string
  /** 输出编码：json / json_lines / text（json_lines = 每行一条记录） */
  encoding: 'json' | 'json_lines' | 'text'
  /** 单条最大字节数（超过则丢弃并记 warn） */
  max_bytes: number
  /** 更新时间 ISO8601 */
  updated_at?: string
}

export interface TemplateVarInfo {
  name: string
  description: string
  /** 数据类型: number / string / bool */
  type: 'number' | 'string' | 'bool'
  /** 必填变量还是可选（聚合类变量为可选） */
  optional?: boolean
}

export interface UplinkTestRequest {
  template: string
  /** 单条测试数据（与 preview 类似，但只测一条） */
  vars: Record<string, string | number | boolean | null>
  /** 是否真的发送（false = 仅渲染不发送） */
  send: boolean
}

export interface UplinkTestResponse {
  ok: boolean
  rendered: string
  bytes: number
  /** 真的发送时才有 */
  publish?: {
    topic: string
    qos: number
    delivered: boolean
    latency_ms: number
  }
  error?: string
}

/** 获取当前上行模板配置 */
export function getUplinkConfig() {
  return request<UplinkConfig>({ url: '/api/uplink/config', method: 'GET' })
}

/** 保存上行模板配置 */
export function saveUplinkConfig(cfg: UplinkConfig) {
  return request<UplinkConfig>({ url: '/api/uplink/config', method: 'POST', data: cfg })
}

/** 列出所有可用模板变量（含说明） */
export function listTemplateVars() {
  return request<TemplateVarInfo[]>({ url: '/api/uplink/vars', method: 'GET' })
}

/** 测试发送：渲染并（可选）真实上行 */
export function testUplink(payload: UplinkTestRequest) {
  return request<UplinkTestResponse>({ url: '/api/uplink/test', method: 'POST', data: payload })
}

/** 内置模板库（按场景预设好的 JSON） */
export const BUILTIN_TEMPLATES: Array<{
  id: string
  name: string
  description: string
  template: string
}> = [
  {
    id: 'default',
    name: '默认 JSON（按设备分组）',
    description: '与历史 payload schema 兼容，按设备嵌套点位数组',
    template: `{
  "ts": \${timestamp_ms},
  "gateway_id": "gateway-001",
  "reports": [
    {
      "device_id": "\${device_id}",
      "points": [
        {
          "point_id": "\${point_id}",
          "value": \${value},
          "quality": "\${quality_text}",
          "ts": \${timestamp_ms}
        }
      ]
    }
  ]
}`,
  },
  {
    id: 'thingsboard',
    name: 'ThingsBoard',
    description: 'ThingsBoard JSON 遥测格式（values 内点位动态作为 key）',
    template: `{"ts":\${timestamp_ms},"values":{"\${point_id}":\${value}}}`,
  },
  {
    id: 'aliyun',
    name: '阿里云 IoT 平台',
    description: '阿里云物模型属性上报格式',
    template: `{"method":"thing.event.property.post","params":{"\${point_id}":\${value}}}`,
  },
  {
    id: 'huawei',
    name: '华为云 IoTDA',
    description: '华为云 IoTDA 设备消息上报',
    template: `{"device_id":"\${device_id}","properties":{"\${point_id}":\${value}},"event_time":\${timestamp_ms}}`,
  },
  {
    id: 'aws-iot',
    name: 'AWS IoT Core',
    description: 'AWS IoT 自定义 topic + payload',
    template: `{"device":"\${device_id}","timestamp":\${timestamp_ms},"metric":"\${point_id}","value":\${value}}`,
  },
  {
    id: 'influxdb',
    name: 'InfluxDB Line Protocol',
    description: 'InfluxDB 行协议（适合直接写时序库）',
    template: `gateway_measurement,device=\${device_id},point=\${point_id} value=\${value} \${timestamp_ms}`,
  },
  {
    id: 'json-lines',
    name: 'JSON Lines（每行一条）',
    description: '便于流式处理的 NDJSON 格式',
    template: `{"ts":\${timestamp_ms},"device":"\${device_id}","point":"\${point_id}","value":\${value}}`,
  },
]
// ===== 模板导入/导出（细粒度：仅模板字符串，不含其他网关配置） =====

export interface UplinkTemplateExport {
  template: string
  enabled: boolean
  encoding: string
  max_bytes: number
  /** 导出时间 ISO8601 */
  exported_at: string
}

export interface UplinkTemplateImport {
  template: string
  enabled: boolean
}

// ===== 测试历史 =====

export interface UplinkTestHistoryItem {
  id: number
  /** 创建时间戳（毫秒） */
  timestamp: number
  template: string
  rendered: string
  /** 输入变量 JSON 字符串 */
  vars_json: string
  send: boolean
  delivered: boolean
  latency_ms: number
  error?: string | null
}

/** 导出当前模板（仅模板字符串与 enabled 状态） */
export function exportTemplate() {
  return request<UplinkTemplateExport>({ url: '/api/uplink/template/export', method: 'GET' })
}

/** 导入模板（编译校验 + 写盘 + 热重载 + 版本快照） */
export function importTemplate(payload: UplinkTemplateImport) {
  return request<UplinkConfig>({ url: '/api/uplink/template/import', method: 'POST', data: payload })
}

/** 列出最近的测试历史（默认 50 条，最多 200 条） */
export function listTestHistory(limit = 50) {
  return request<UplinkTestHistoryItem[]>({ url: `/api/uplink/test/history?limit=${limit}`, method: 'GET' })
}

/** 清空所有测试历史 */
export function clearTestHistory() {
  return request<{ cleared: boolean }>({ url: '/api/uplink/test/history', method: 'DELETE' })
}

/** B11 透传会话（自 network.ts 归拢） */
export interface PassthroughSession { id: string; bus: string; started_at: number; timeout_ms: number }
export async function getPassthroughSessions(): Promise<{enabled: boolean; sessions: PassthroughSession[]}> {
  return request({ url: '/api/passthrough/sessions', method: 'GET' })
}

/** C1 北向通道状态（自 network.ts 归拢） */
export interface NorthChannelExtra { broker?: string; circuit_open?: boolean; url?: string; success_count?: number; failed_count?: number; avg_latency_ms?: number | null; topic?: string; build_supported?: boolean; /** 最近一次失败原因（连接器真实记录，成功后清空） */ last_error?: string }
export interface NorthChannel { name: string; type: string; enabled: boolean; connected: boolean | null; queue_length?: number; extra: NorthChannelExtra }
export interface NorthAggregated { mqtt: number; http: number; mqtt_connected: number; http_connected: number; pending_messages: number; total_messages_sent: number; /** 扇出订阅 Lagged 累计丢帧批次数 */ lagged_dropped_batches?: number }
/** C2 北向 Server 存活状态（OPC UA / Modbus / 透传 Server） */
export interface NorthServer { name: string; running: boolean }
/**
 * 第 1 期冗余收口：原 `GET /api/north/servers` 已删除，
 * 其内容并入 `GET /api/north/status` 的 `servers` 字段。
 */
export interface NorthStatus { healthy: boolean; channels: NorthChannel[]; aggregated: NorthAggregated; servers: NorthServer[] }
export function getNorthStatus(): Promise<NorthStatus> {
  return request({ url: '/api/north/status', method: 'GET' })
}