import { Connection } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 点位菜单组 — 点位管理 Hub（原「地址换算」Hub 已并入其「进阶工具」分组）。
 *  原「点位模板」独立入口已并入「设备模板」（同一后端 /api/templates 实体）。
 */
export const pointsGroup: MenuGroup = {
  name: '点位',
  icon: Connection,
  items: [
    { title: '点位管理', path: '/points', icon: Connection },
  ],
}
