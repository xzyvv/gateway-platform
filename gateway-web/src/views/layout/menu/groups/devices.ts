import { Cpu, Share, MagicStick } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 设备菜单组 — 设备域全部入口：
 *  - 快速接入：总线 → 设备 → 点位连续读取的直线向导（跳转部署向导页签，新用户首站）
 *  - 设备管理 Hub：设备清单 / 设备分组 / 批量命令 / 健康评分 / 状态快照 / 远程控制 /
 *    设备模板 / 批量导入 / 设备发现（原独立「设备运维」Hub 已并入）
 *  - 设备拓扑：独立页面（原挂在概览组，按业务域归位）
 *
 *  设备批量导入 / 批量命令已接入真实 API，回归本 Hub（设备管理 › 批量命令 / 设备批量导入）。
 */
export const devicesGroup: MenuGroup = {
  name: '设备',
  icon: Cpu,
  items: [
    { title: '快速接入', path: '/devices/quick-setup', icon: MagicStick, description: '跳转到配置管理的部署向导（总线、设备、点位三步接入）' },
    { title: '设备管理', path: '/devices', icon: Cpu },
    { title: '设备拓扑', path: '/devices/topology', icon: Share },
  ],
}
