/**
 * 诊断运维 View-Friendly 类型定义（camelCase + number 时间戳）
 *
 * 从 ./view 抽出的纯类型/常量声明：后端 ts-rs 把 i64/u64 映射为 bigint，
 * 且字段名为 snake_case；View 层用 camelCase + number 时间戳，这里集中声明适配后的形态。
 * 由 ./view 重新导出，保持 `from '@/api/diagnostics'` 调用方兼容。
 */

export interface BusLoadHistoryView {
  time: number
  tx: number
  rx: number
}

export interface BusLoadView {
  bus: string
  type: string
  /** null = 首次采样无基线（或区间内无采集），UI 渲染「—」 */
  txRate: number | null
  rxRate: number | null
  collisionRate: number | null
  avgLatency: number | null
  /** null = 队列为全局单队列，无法按总线拆分 */
  backlog: number | null
  utilization: number | null
  history: BusLoadHistoryView[]
}

export interface ModbusErrorView {
  code: string
  name: string
  meaning: string
  cause: string
  solution: string
}

export interface CommonIssueView {
  symptom: string
  checks: string[]
}

export interface DeviceStatView {
  deviceId: string
  deviceName: string
  protocol: string
  totalReads: number
  success: number
  failed: number
  /** null = 窗口内无采集，UI 渲染「—」 */
  successRate: number | null
  avgLatency: number | null
  dataPoints: number
}

export interface BusStatView {
  bus: string
  protocol: string
  totalReads: number
  success: number
  failed: number
  /** null = 窗口内无采集，UI 渲染「—」 */
  successRate: number | null
  avgLatency: number | null
  deviceCount: number
  /** 在线设备数（最近一次采集成功的设备数） */
  onlineDevices: number
  /** 超时率（%）；null = 窗口内无采集 */
  timeoutRate: number | null
  /** 最近一次失败的错误码，如 TIMEOUT / CRC / EXC_02；无失败为 null */
  lastErrorCode: string | null
  /** 重连次数（自进程启动以来累计） */
  reconnectCount: number
}

export interface ErrorDistItemView {
  errorType: string
  count: number
  percentage: number
}

export interface StatsSummaryView {
  totalReads: number
  /** null = 窗口内无采集，UI 渲染「—」 */
  successRate: number | null
  avgLatency: number | null
  totalDataPoints: number
}

export interface StatsReportView {
  period: string
  startAt: number
  endAt: number
  deviceStats: DeviceStatView[]
  busStats: BusStatView[]
  errorDist: ErrorDistItemView[]
  summary: StatsSummaryView
}

export interface DiagStepView {
  name: string
  status: 'pass' | 'fail' | 'warning' | 'pending'
  message: string
  detail?: string
}

export interface DiagResultView {
  pointId: string
  pointName: string
  status: 'healthy' | 'degraded' | 'offline'
  steps: DiagStepView[]
  suggestion: string
  lastCheckAt: number
}

export interface NetInterfaceView {
  name: string
  ip: string
  status: 'up' | 'down'
  rx: number
  tx: number
}

export interface SerialPortView {
  name: string
  status: 'ok' | 'error'
  desc: string
}

export interface GatewayDiagView {
  /** null = 该平台不支持或读取失败，UI 渲染为「—」 */
  cpuTemp: number | null
  cpuUsage: number | null
  memUsage: number | null
  diskUsage: number | null
  /** 主机开机时长（人类可读；读取失败为「—」） */
  hostUptime: string
  /** gatewayd 进程运行时长（人类可读） */
  processUptime: string
  netInterfaces: NetInterfaceView[]
  serialPorts: SerialPortView[]
  loadAvg: number | null
  processes: number | null
}

/** 诊断步骤状态（与 DiagStepView['status'] 一致） */
export type DiagStepStatus = DiagStepView['status']

/**
 * N6: 诊断页面原先用这组写死的示例点位。现改为「设备 → 真实点位」两级选择
 * （数据来自 /api/devices 与 /api/devices/:id/points），示例点位已无调用方，
 * 保留会让后来者误以为诊断仍基于假数据，故移除。
 */
