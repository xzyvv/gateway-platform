import { HomeFilled, DataLine } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 概览菜单组：只放网关级门面页；设备拓扑归「设备」组，监控类入口归「运维」组。
 *  组图标沿用全站模式：= 组内第一个主项（运行总览）的图标。 */
export const overviewGroup: MenuGroup = {
  name: '概览',
  icon: HomeFilled,
  items: [
    { title: '运行总览', path: '/overview', icon: HomeFilled },
    { title: '实时数据', path: '/overview/realtime', icon: DataLine },
  ],
}
