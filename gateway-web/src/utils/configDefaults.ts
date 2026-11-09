/**
 * 运行参数页的纯数据骨架与默认值
 *
 * 从 useConfigActions 抽出的无副作用工厂/常量：表单类型、顶层已知键集合、
 * Web/MQTT 节的 serde 默认值、表单构造种子、未知顶层键检测。
 * 不依赖 Vue，可被 composable 与测试直接复用。
 */
import type { GatewayConfig, MqttConfig, SimConsumerConfig, WebConfig } from '@/api/types'

// 表单态即 API 形态（与后端 ts-rs 生成的 GatewayConfig 对齐）：
// buses 为无名数组，总线身份即 port_name，与后端 Vec<BusConfig> 一致。
// 额外的字符串索引签名仅为兼容「原始 JSON」标签整体 Object.assign 导入未知节。
export type ConfigForm = GatewayConfig & { [key: string]: unknown }

// B2: 顶层已知键集合（与 src/api/generated/GatewayConfig.ts 字段列表一致）。
// 同步约束：后端 ts-rs 增/删顶层字段时同步维护本集合（不维护会导致拼写错的未知键
// 被静默接受、模板/原始 JSON 整段导入时无声丢失关键节）。
export const KNOWN_GATEWAY_TOP_KEYS = new Set<string>([
  'schema_version',
  'buses', 'points', 'devices', 'point_trash',
  'mqtt', 'ntp', 'web', 'webhook',
  'sim_consumer',
  'cellular', 'opcua',
  'modbus_server', 'modbus_passthrough',
  'device_groups', 'device_assets', 'virtual_points',
  'telemetry', 'license',
  'self_heal',
  'queue',
])

// B1: 其余 nullable 节（后端 serde 默认 null）若用户解析的 JSON 漏写，
// Object.assign 不会写入，原值保留为 null 不算丢失；但「重置为最小骨架后
// 用户先编辑再解析 JSON」的路径下，parsed 里缺这些键会保持上一轮状态——
// 解析时需显式归一化为 null，避免历史脏值混入。
export const KNOWN_NULLABLE_KEYS = [
  'devices', 'ntp', 'webhook', 'cellular', 'opcua',
  'modbus_server', 'modbus_passthrough', 'device_groups', 'device_assets',
  'virtual_points', 'telemetry', 'license', 'point_trash',
] as const

// 后端 WebConfig 的 serde 默认值（web 节缺失时兜底，工厂函数保证每次返回新对象）
//
// listen_addr / allow_anon_read 必须与后端 `gateway-core/src/config/web.rs` 的
// serde 默认值一致：兜底填 `0.0.0.0:8080` 会让「配置里缺 web 段 → 前端补默认值
// → 保存回写」这条路径产出一份绑定全网卡 + 空 password_hash 的配置，
// 而这正是 bootstrap 的 fail-closed 硬性违规，保存后网关重启即被拒绝启动。
export function defaultWeb(): WebConfig {
  return {
    api_token: '',
    username: 'admin',
    password_hash: '',
    listen_addr: '127.0.0.1:8080',
    allow_anon_read: false,
    realtime_buffer: 1024,
    realtime_max_subscribers: 64,
    ota_signing_key: null,
    rate_limit_per_sec: null,
    cors_origins: null,
    allow_default_token: true,
    users: [],
    webhook_test_allow_hosts: [],
    webhook_test_block_private: false,
    demo_replay_sessions: false,
  }
}

// 后端 MqttConfig 的 serde 默认值（mqtt 节缺失时兜底，工厂函数保证每次返回新对象）
export function defaultMqtt(): MqttConfig {
  return { enabled: false, broker: '', topic: '', publish_interval: 60, qos: 1, keep_alive: 30, ack_timeout_secs: 10, username: null, password: null, compression: null, compression_threshold: null, resume_rate_limit: null, payload_template: null, tls: null, reconnect_backoff: { seconds: [1, 2, 5, 10, 30], jitter_ratio: 0 }, report_mode: 'stream' }
}

// 后端 SimConsumerConfig 的 serde 默认值（sim_consumer 节缺失时兜底，工厂函数保证每次返回新对象）
export function defaultSimConsumer(): SimConsumerConfig {
  return { enabled: false, batch_size: 100, interval_ms: 1000, log_every: 100 }
}

/** 检测未识别的顶层键，返回键名列表（不触发任何 UI 副作用，由调用方决定如何提示） */
export function unknownTopKeys(source: Record<string, unknown>): string[] {
  return Object.keys(source).filter((k) => !KNOWN_GATEWAY_TOP_KEYS.has(k))
}

/** 表单构造种子：顶层键的最小骨架（键顺序与 GatewayConfig 字段列表保持一致） */
export function createSeedConfig(): ConfigForm {
  return {
    schema_version: 1,
    buses: [],
    points: [],
    devices: null,
    point_trash: undefined,
    mqtt: defaultMqtt(),
    sim_consumer: defaultSimConsumer(),
    ntp: null,
    web: defaultWeb(),
    webhook: null,
    cellular: null,
    opcua: null,
    modbus_server: null,
    modbus_passthrough: null,
    device_groups: null,
    device_assets: null,
    virtual_points: null,
    telemetry: null,
    license: null,
    self_heal: null,
    queue: { max_rows: 1_000_000 },
  }
}