/**
 * 设备拓扑图 API
 *
 * 路由：
 * - GET /api/devices/topology -> getTopology
 *
 * 第 1 期冗余收口：原 `GET /api/devices/topology/stats` 是本端点的聚合派生
 * （同配置同判据），后端已删除该端点，统计并入响应内的 `stats` 字段。
 */
import { request } from '../index'
import type { TopoNode as TopoNodeRaw } from '../generated/TopoNode'
import type { TopoLink as TopoLinkRaw } from '../generated/TopoLink'
import type { TopoStats as TopoStatsRaw } from '../generated/TopoStats'
import type { TopologyData as TopologyDataRaw } from '../generated/TopologyData'

/**
 * 视图类型从 ts-rs generated 契约派生：字段名以契约为真源，
 * 只把 node_type 从契约的 string 收窄为本页已知的字面量联合（渲染时按类型分支）。
 */
export type TopoNode = Omit<TopoNodeRaw, 'node_type'> & {
  node_type: 'bus' | 'device' | 'point'
}

// 以下两者与 generated 字段完全一致，直接复用契约类型
export type TopoLink = TopoLinkRaw
export type TopoStats = TopoStatsRaw

/** 顶层响应复用契约，仅把 nodes 换成上面收窄过的 TopoNode */
export type TopologyData = Omit<TopologyDataRaw, 'nodes'> & { nodes: TopoNode[] }

export function getTopology(): Promise<TopologyData> {
  return request<TopologyData>({ url: '/api/devices/topology', method: 'GET' })
}