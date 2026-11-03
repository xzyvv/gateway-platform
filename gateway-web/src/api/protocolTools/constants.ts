/**
 * 协议工具静态常量与字面量联合类型
 *
 * PROTO_LABEL，以及后端用 String 表达、前端用字面量联合类型约束的枚举字段。
 *
 * 注意：
 * - 总线列表不再有前端静态常量——真实总线来自 GET /api/config 的 buses，
 *   统一经 useDebugTargets() 加载（value 为 port_name）。
 * - 预置帧（presets）不再有前端副本——由 `listPresets()` 从后端读取（P3-4）。
 */

// === 常量 ===
export const PROTO_LABEL: Record<string, string> = {
  'modbus-tcp': 'Modbus TCP', 'modbus-rtu': 'Modbus RTU', mqtt: 'MQTT', 'opc-ua': 'OPC UA',
}

// === 类型联合（后端用 String，前端用字面量联合类型）===
export type RegType = 'holding' | 'input' | 'coils' | 'discrete'
export type ProtoType = 'modbus-tcp' | 'modbus-rtu' | 'mqtt' | 'opc-ua'
export type TestStatus = 'pass' | 'fail' | 'skip' | 'pending'