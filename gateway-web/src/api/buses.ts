import { request } from './index'

/**
 * 总线采集暂停/恢复（E1）
 *
 * 协议会话、报文回放、从站扫描等调试能力要求「总线未被采集引擎占用」。
 * 系统原先没有暂停入口，用户只会拿到「请先暂停该总线的采集后再试」这条
 * 做不到的指引；这三个接口把这条死路补成可执行动作。
 */
export interface BusPauseResult {
  bus: string
  paused: boolean
  /** 当前全部处于暂停状态的总线（调试页刷新后据此还原状态） */
  paused_buses: string[]
}

/** 暂停某条总线的采集（不修改配置，仅从调度中临时摘除该总线点位） */
export function pauseBusCollect(bus: string): Promise<BusPauseResult> {
  return request<BusPauseResult>({
    url: `/api/buses/${encodeURIComponent(bus)}/pause`,
    method: 'POST',
  })
}

/** 恢复某条总线的采集（按当前配置重建该总线的采集组） */
export function resumeBusCollect(bus: string): Promise<BusPauseResult> {
  return request<BusPauseResult>({
    url: `/api/buses/${encodeURIComponent(bus)}/resume`,
    method: 'POST',
  })
}

/** 当前暂停中的总线列表 */
export function listPausedBuses(): Promise<{ paused_buses: string[] }> {
  return request<{ paused_buses: string[] }>({ url: '/api/buses/paused', method: 'GET' })
}
