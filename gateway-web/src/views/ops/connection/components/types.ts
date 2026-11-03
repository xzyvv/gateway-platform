import type { DeviceConfig } from '@/api/types'
import type { NorthChannel } from '@/api/uplink'

/** 扩展设备行：附加测试状态字段（容器维护，子组件只读渲染） */
export interface DeviceRow extends DeviceConfig {
  _status?: 'idle' | 'testing' | 'ok' | 'fail'
  _latency?: number
  _error?: string
}

/** 扩展通道行：附加测试状态字段 */
export interface ChannelRow extends NorthChannel {
  _status?: 'idle' | 'testing' | 'ok' | 'fail'
  _latency?: number
  _error?: string
  _detail?: string
}

/** 测试结果汇总统计（容器 computed，传入 TestSummaryPanel） */
export interface TestSummary {
  total: number
  pass: number
  fail: number
  avgLatency: number
  lastAt: string
}