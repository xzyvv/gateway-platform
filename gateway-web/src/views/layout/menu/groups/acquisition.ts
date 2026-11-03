import { Operation, Tools, Stopwatch } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 采集菜单组 — 协议读写 / 统一协议调试台 / 采集调优。 */
export const acquisitionGroup: MenuGroup = {
  name: '采集',
  icon: Operation,
  items: [
    { title: '点位读写', path: '/acquisition/tester', icon: Operation },
    // 第三阶段 C1：原「协议扫描 / 协议仿真 / 报文调试」三个入口合并为一个调试台，
    // 先进台选总线目标，再在左侧分组页签里选动作（扫描 / 报文 / 模拟与扩展）
    { title: '协议调试台', path: '/acquisition/console', icon: Tools, description: '扫描 / 报文 / 模拟一站式调试，先选总线目标再选动作' },
    { title: '采集调优', path: '/acquisition/comm-tuning', icon: Stopwatch },
  ],
}
