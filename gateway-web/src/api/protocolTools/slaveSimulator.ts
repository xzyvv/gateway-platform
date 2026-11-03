/**
 * 从机模拟器 API（真实 Modbus TCP 从站）
 * GET  /api/protocol-tools/simulator/registers
 * PUT  /api/protocol-tools/simulator/registers/:addr
 * POST /api/protocol-tools/simulator/start
 * POST /api/protocol-tools/simulator/stop
 * GET  /api/protocol-tools/simulator/status
 *
 * start 可带 port（缺省 1502，0 表示系统分配）；isRunning 为便捷包装。
 */
import { request } from '../index'
import type { SimRegister, SimulatorStatus } from './types'
import type { SimRegister as SimRegisterRaw, SimulatorStatus as SimulatorStatusRaw } from '../generated'
import { toSimRegister, toSimulatorStatus } from './types'

export function getRegisters(): Promise<SimRegister[]> {
  return request<SimRegisterRaw[]>({ url: '/api/protocol-tools/simulator/registers', method: 'GET' }).then((arr) => arr.map(toSimRegister))
}
export function updateRegister(addr: number, value: number): Promise<void> {
  return request<void>({ url: '/api/protocol-tools/simulator/registers/' + addr, method: 'PUT', data: { value } })
}
export function startSimulate(slaveId: number, port?: number): Promise<void> {
  return request<void>({ url: '/api/protocol-tools/simulator/start', method: 'POST', data: { slave_id: slaveId, port } })
}
export function stopSimulate(): Promise<void> {
  return request<void>({ url: '/api/protocol-tools/simulator/stop', method: 'POST' })
}
export function getSimulatorStatus(): Promise<SimulatorStatus> {
  return request<SimulatorStatusRaw>({ url: '/api/protocol-tools/simulator/status', method: 'GET' }).then(toSimulatorStatus)
}
export async function isRunning(): Promise<boolean> {
  const s = await getSimulatorStatus()
  return s.running
}
