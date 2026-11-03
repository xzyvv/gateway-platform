import { Histogram } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 数据菜单组 — 数据流水 Hub（原「数据质量」Hub 已并入其「质量与治理」分组）。
 *  - 数据查询：历史数据 / 区间对比
 *  - 传输与压缩：数据压缩 / 断连续传 / 链路追踪
 *  - 质量与治理：质量概览 / 采集退避
 *    （「数据插值」与「基线对比」页签已下线：前者只在浏览器内计算、不写回网关；
 *      后者的基线值仅存 localStorage，网关不加载不执行）
 *  - 原「快照对比 / 导出格式 / 数据清洗」三个影子页签已删除
 */
export const dataGroup: MenuGroup = {
  name: '数据',
  icon: Histogram,
  items: [
    { title: '数据流水', path: '/data', icon: Histogram, description: '历史/压缩/断连续传/链路追踪/对比/质量/退避' },
  ],
}
