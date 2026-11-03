/**
 * 内置模拟消费者 API（网关真实后端：/api/sim_consumer/status|start|stop）
 *
 * 模拟消费者是上行通道的兜底消费者：MQTT 不可用时把队列数据「出队 → 处理 → ack」
 * 走完一整圈，消除「队列有积压却没有消费者」的告警歧义，但不会外发任何数据。
 *
 * 与 MQTT **互斥**：两者会分流同一批队列数据，被模拟消费者分走的那部分被消费掉却
 * 不会上报，现场表现为静默丢数。因此 MQTT 已启用时后端拒绝启动（HTTP 409）。
 *
 * 状态字段口径：
 * - `enabled` = 配置意图（[sim_consumer].enabled，启动时是否自动拉起）
 * - `running` = 实际运行态（run 循环是否在跑）
 */
import { request } from './index'
import type { SimConsumerStatus } from './types'

/** 启动参数（后端 camelCase 接收；字段均可选，缺省沿用后端当前值） */
export interface SimConsumerStartParams {
  /** 单批出队条数 */
  batchSize?: number
  /** 队列为空时的轮询间隔（毫秒） */
  intervalMs?: number
  /** 每消费满多少条打一条 INFO 摘要日志（0 = 不打） */
  logEvery?: number
}

/** 状态快照（配置意图 + 实际运行态 + 计数 + 队列深度） */
export function getSimConsumerStatus(): Promise<SimConsumerStatus> {
  return request<SimConsumerStatus>({ url: '/api/sim_consumer/status', method: 'GET' })
}

/**
 * 启动模拟消费者。
 *
 * 运行参数只在**未运行**时可改：运行中调用后端返回 400
 * （「sim consumer 正在运行：运行参数不可修改，请先停止再修改」）。
 */
export function startSimConsumer(params: SimConsumerStartParams = {}): Promise<SimConsumerStatus> {
  return request<SimConsumerStatus>({
    url: '/api/sim_consumer/start',
    method: 'POST',
    data: { ...params },
  })
}

/** 停止模拟消费者（等待 run 循环真正退出后返回） */
export function stopSimConsumer(): Promise<SimConsumerStatus> {
  return request<SimConsumerStatus>({ url: '/api/sim_consumer/stop', method: 'POST' })
}
