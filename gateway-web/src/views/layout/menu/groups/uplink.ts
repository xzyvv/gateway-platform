import { Upload } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 上行菜单组——聚合后仅保留 Hub 入口。 */
export const uplinkGroup: MenuGroup = {
  name: '上行',
  icon: Upload,
  items: [
    { title: '上行转发', path: '/uplink', icon: Upload },
  ],
}