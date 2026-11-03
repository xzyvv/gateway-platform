import { TrendCharts } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 运维菜单组 — 运维监控 Hub（原「性能监控 / 链路诊断 / 健康巡检」三个 Hub 已合并）。
 *  组内分组：运行观察 / 时序分析 / 连通诊断 / 资源 / 自诊断与报告。
 *  收藏监控独立页已下线，入口并入「运行总览 › 实时数据」的「仅看收藏」。 */
export const opsGroup: MenuGroup = {
  name: '运维',
  icon: TrendCharts,
  items: [
    { title: '运维监控', path: '/ops', icon: TrendCharts, description: '总线负载 / 采集诊断与统计 / 采集甘特图 / 连接测试 / 磁盘健康 / 自诊断与健康报告' },
  ],
}
