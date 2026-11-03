// 网关 API 类型定义
// 注意：以下类型由后端 Rust struct 通过 ts-rs 自动生成，位于 ./generated/。
// 后端类型变更后运行 `cargo test export_ts_types` 同步。

export type {
  BusConfig,
  OpcUaClientConfig,
  PointConfig,
  DeviceConfig,
  RunStatus,
  VersionInfo,
  OtaStatus,
  ConfigVersion,
  AggFn,
  AggregationConfig,
  GatewayConfig,
  MqttConfig,
  WebConfig,
  WebhookConfig,
  OpcuaConfig,
  OpcuaUserConfig,
  ModbusServerConfig,
  ModbusPassthroughConfig,
  CellularConfig,
  NtpConfig,
  SelfHealConfig,
  SimConsumerConfig,
  SimConsumerStatus,
} from './generated'

/** 实时数据点（前端专用，后端 DataPoint 暂未接入 ts-rs） */
export interface DataPoint {
  point_id: string
  device_id: string
  value: number
  raw_value: number
  raw_type: string
  quality: string
  timestamp: number
}

/**
 * /health/ready 响应（该端点尚未接入 ts-rs，前端侧定义）。
 * components 为组件名 → 状态字符串（ok/error/...）的映射。
 */
export interface HealthReady {
  status: string
  components: Record<string, string>
  /** 采集队列积压条数（后端 status.rs 随快照返回；老版本无此字段） */
  queue_len?: number
}

/** 组件健康条目：HealthReady.components 展开后的表格行结构 */
export interface HealthComponent {
  name: string
  status: string
}

/** 通用 API 响应 */
export interface ApiResponse<T = unknown> {
  status?: string
  data?: T
  error?: string
  [key: string]: unknown
}
