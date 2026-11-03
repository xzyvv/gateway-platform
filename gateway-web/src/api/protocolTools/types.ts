/**
 * 协议工具 View 接口 + Raw → View 转换函数
 *
 * 后端 ts-rs 生成类型为 snake_case + bigint 时间戳，
 * 这里定义 camelCase + number 时间戳的 View 接口供 UI 使用，
 * 并提供 toXxx 转换函数（snake_case → camelCase，bigint → number）。
 * 透传无字段名差异的类型直接 re-export from generated。
 */
import type {
  AutoDetectResult as AutoDetectResultRaw,
  ConvertEnumItem,
  DetectStep,
  Packet as PacketRaw,
  SendResult as SendResultRaw,
  ReplayPacket as ReplayPacketRaw,
  ReplayExecuteResult as ReplayExecuteResultRaw,
  ReplayFrameResult as ReplayFrameResultRaw,
  ReplaySession as ReplaySessionRaw,
  ConvertRule as ConvertRuleRaw,
  ScanCell as ScanCellRaw,
  ScanResult as ScanResultRaw,
  PointSuggestion as PointSuggestionRaw,
  SlaveScanResult as SlaveScanResultRaw,
  SimRegister as SimRegisterRaw,
  SimulatorStatus as SimulatorStatusRaw,
} from '../generated'

// 透传：字段名与 View 兼容，无需重新定义
export type { DetectStep, ConvertEnumItem }

/** 参数探测任务（p0-5 契约 §4）：steps 逐项 pending/testing/success/fail */
export interface AutoDetectResult {
  id: string; deviceId: string; deviceName: string; bus: string; slaveId: number;
  steps: DetectStep[]; status: string; progress: number;
  startTime: number; endTime?: number | null;
}

export interface Packet {
  id: string; time: number; direction: 'request' | 'response';
  deviceId: string; deviceName: string; bus: string;
  hex: string; ascii: string; parsed: string;
  matchedId?: string; delayMs?: number;
}

export interface SendResult {
  requestHex: string; responseHex: string; success: boolean; latencyMs: number; parsed: string;
  /** 失败分类（timeout/crc/exception/bus_offline/bus_busy/invalid_frame/bus_not_found）；成功时为 null */
  error?: string | null;
}

export interface ReplayPacket { id: string; time: number; direction: 'request' | 'response'; hex: string; parsed: string }
export interface ReplaySession {
  id: string; name: string; deviceName: string;
  durationMs: number; packetCount: number; createdAt: number;
  packets: ReplayPacket[];
}

/** 单帧回放结果（契约 §4） */
export interface ReplayFrameResult {
  packetId: string; requestHex: string; expectedResponseHex: string; actualResponseHex: string;
  /** match（与录制一致）/ mismatch（有响应但不同）/ timeout（无响应）/ error（发送失败） */
  status: string; latencyMs: number; error?: string | null;
}

/** 回放执行结果（契约 §4）：match/mismatch 是后端 JSON 字段名，View 侧沿用原名 */
export interface ReplayExecuteResult {
  sessionId: string; bus: string;
  startedAtMs: number; finishedAtMs: number;
  total: number; match: number; mismatch: number; failed: number;
  results: ReplayFrameResult[];
}

export interface ConvertRule {
  id: string; name: string; sourceProto: string; sourcePointId: string; sourcePointName: string;
  targetProto: string; targetTag: string; transform: string;
  scale?: number | null; offset?: number | null;
  enumMap?: ConvertEnumItem[] | null;
  enabled: boolean; lastConvertedAt?: number | null; lastValue?: string | null; createdAt: number;
}

export interface ScanCell { address: number; responded: boolean; value?: number | null; funcCode?: number | null; error?: string | null }
export interface ScanResult {
  id: string; bus: string; slaveId: number; regType: string;
  startAddr: number; endAddr: number; total: number; responded: number;
  cells: ScanCell[]; createdAt: number; status: string; progress: number;
}
export interface PointSuggestion { id: string; name: string; address: string; address_num: number }

export interface SlaveScanResult { slaveId: number; responded: boolean; deviceName?: string | null; funcCode?: string | null; latencyMs?: number | null }

/** 流式扫描 started 事件（p0-2 契约 §4.1） */
export interface SlaveScanStarted {
  bus: string; rangeStart: number; rangeEnd: number; total: number;
  timeoutMs: number; intervalMs: number; startedAtMs: number
}
/** 流式扫描 summary 事件（p0-2 契约 §4.3）；noise>0 提示线路有干扰 */
export interface SlaveScanSummary { bus: string; scanned: number; found: number; noise: number; durationMs: number; results: SlaveScanResult[] }

export interface SimRegister { addr: number; name: string; value: number; type: string; unit?: string | null }

/** Modbus TCP 从站运行状态（port=0 启动时 port 为系统实际分配值） */
export interface SimulatorStatus {
  running: boolean; slaveId: number | null; port: number;
  listenAddr: string | null; requests: number; errors: number; startedAtMs: number | null;
}

// Preset 由 ts-rs 生成，View 直接复用
export type { Preset } from '../generated'

// === Raw → View 转换函数 ===
// snake_case → camelCase，bigint 时间戳 → number，
// null → undefined（仅在 View 接口允许 undefined 字段时）。

export function toPacket(p: PacketRaw): Packet {
  return { id: p.id, time: Number(p.time_ms), direction: p.direction as 'request' | 'response',
    deviceId: p.device_id, deviceName: p.device_name, bus: p.bus, hex: p.hex, ascii: p.ascii, parsed: p.parsed,
    matchedId: p.matched_id ?? undefined, delayMs: p.delay_ms ?? undefined }
}

export function toSendResult(r: SendResultRaw): SendResult {
  return { requestHex: r.request_hex, responseHex: r.response_hex, success: r.success, latencyMs: r.latency_ms, parsed: r.parsed, error: r.error }
}

/** 仅本文件内部使用（Raw → View） */
function toReplayPacket(p: ReplayPacketRaw): ReplayPacket {
  return { id: p.id, time: Number(p.time_ms), direction: p.direction as 'request' | 'response', hex: p.hex, parsed: p.parsed }
}

export function toReplaySession(s: ReplaySessionRaw): ReplaySession {
  return { id: s.id, name: s.name, deviceName: s.device_name, durationMs: s.duration_ms,
    packetCount: s.packet_count, createdAt: Number(s.created_at_ms), packets: s.packets.map(toReplayPacket) }
}

/** 仅本文件内部使用（View → Raw，保存会话时 POST body 用；后端 serde 按 snake_case 反序列化，缺字段会 400） */
function fromReplayPacket(p: ReplayPacket): ReplayPacketRaw {
  return { id: p.id, time_ms: p.time, direction: p.direction, hex: p.hex, parsed: p.parsed }
}

export function fromReplaySession(s: ReplaySession): ReplaySessionRaw {
  return { id: s.id, name: s.name, device_name: s.deviceName, duration_ms: s.durationMs,
    packet_count: s.packetCount, created_at_ms: s.createdAt, packets: s.packets.map(fromReplayPacket) }
}

export function toReplayFrameResult(r: ReplayFrameResultRaw): ReplayFrameResult {
  return { packetId: r.packet_id, requestHex: r.request_hex, expectedResponseHex: r.expected_response_hex,
    actualResponseHex: r.actual_response_hex, status: r.status, latencyMs: r.latency_ms, error: r.error }
}

export function toReplayExecuteResult(r: ReplayExecuteResultRaw): ReplayExecuteResult {
  return { sessionId: r.session_id, bus: r.bus, startedAtMs: Number(r.started_at_ms), finishedAtMs: Number(r.finished_at_ms),
    total: r.total, match: r.match, mismatch: r.mismatch, failed: r.failed,
    results: r.results.map(toReplayFrameResult) }
}

export function toConvertRule(r: ConvertRuleRaw): ConvertRule {
  return { id: r.id, name: r.name, sourceProto: r.source_proto, sourcePointId: r.source_point_id,
    sourcePointName: r.source_point_name, targetProto: r.target_proto, targetTag: r.target_tag,
    transform: r.transform, scale: r.scale, offset: r.offset, enumMap: r.enum_map,
    enabled: r.enabled, lastConvertedAt: r.last_converted_at_ms != null ? Number(r.last_converted_at_ms) : null,
    lastValue: r.last_value, createdAt: Number(r.created_at_ms) }
}

export function toAutoDetectResult(r: AutoDetectResultRaw): AutoDetectResult {
  return { id: r.id, deviceId: r.device_id, deviceName: r.device_name, bus: r.bus, slaveId: r.slave_id,
    steps: r.steps, status: r.status, progress: r.progress, startTime: Number(r.start_time_ms),
    endTime: r.end_time_ms != null ? Number(r.end_time_ms) : null }
}

/** 仅本文件内部使用（扫描单元格 Raw → View） */
function toScanCell(c: ScanCellRaw): ScanCell {
  return { address: c.address, responded: c.responded, value: c.value, funcCode: c.func_code, error: c.error }
}

export function toScanResult(r: ScanResultRaw): ScanResult {
  return { id: r.id, bus: r.bus, slaveId: r.slave_id, regType: r.reg_type,
    startAddr: r.start_addr, endAddr: r.end_addr, total: r.total, responded: r.responded,
    cells: r.cells.map(toScanCell), createdAt: Number(r.created_at_ms), status: r.status, progress: r.progress }
}

export function toPointSuggestion(s: PointSuggestionRaw): PointSuggestion {
  return { id: s.id, name: s.name, address: s.address, address_num: s.address_num }
}

export function toSlaveScanResult(r: SlaveScanResultRaw): SlaveScanResult {
  return { slaveId: r.slave_id, responded: r.responded, deviceName: r.device_name, funcCode: r.func_code, latencyMs: r.latency_ms }
}

export function toSimRegister(r: SimRegisterRaw): SimRegister {
  return { addr: r.addr, name: r.name, value: r.value, type: r.type, unit: r.unit }
}

export function toSimulatorStatus(s: SimulatorStatusRaw): SimulatorStatus {
  return { running: s.running, slaveId: s.slave_id, port: s.port, listenAddr: s.listen_addr,
    requests: Number(s.requests), errors: Number(s.errors),
    startedAtMs: s.started_at_ms != null ? Number(s.started_at_ms) : null }
}