/**
 * 设备拓扑图 API
 *
 * 路由：
 * - GET /api/devices/topology -> getTopology
 *
 * 第 1 期冗余收口：原 `GET /api/devices/topology/stats` 是本端点的聚合派生
 * （同配置同判据），后端已删除该端点，统计并入响应内的 `stats` 字段。
 */
import { request } from './index'

export interface TopoNode {
  id: string
  name: string
  node_type: 'bus' | 'device' | 'point'
  status: string | null
  meta: string | null
}

export interface TopoLink {
  source: string
  target: string
}

export interface TopoStats {
  bus_count: number
  device_count: number
  online_count: number
  offline_count: number
  warning_count: number
  point_count: number
}

export interface TopologyData {
  nodes: TopoNode[]
  links: TopoLink[]
  /** 拓扑聚合统计（与本响应同源同判据） */
  stats: TopoStats
}

export function getTopology(): Promise<TopologyData> {
  return request<TopologyData>({ url: '/api/devices/topology', method: 'GET' })
}