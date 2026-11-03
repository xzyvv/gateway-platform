import { InfoFilled, Setting, Document } from '@element-plus/icons-vue'
import type { MenuGroup } from '../types'

/** 系统菜单组——聚合后仅保留 Hub 入口。
 *
 * 「运行参数」与「固件与证书」已并入对应 Hub 作为页签（原独立菜单入口已下线）：
 *  - 运行参数 → 配置管理 Hub 的「运行参数」页签
 *  - 固件与证书 → 系统设置 Hub 的「固件升级 / 证书签发」页签
 * 「运行日志」原为独立 Hub，已并入系统设置 Hub 的「日志」分组，菜单入口保留同一路径的页签。
 * 「数据加密」独立页已删除（F-09：base64 冒充加密、轮换为空操作），
 * 其真实可展示的部分改为「系统设置 › 审计与维护」里的「密钥脱敏状态」卡片。
 */
export const systemGroup: MenuGroup = {
  name: '系统',
  icon: InfoFilled,
  items: [
    { title: '系统设置', path: '/system', icon: InfoFilled },
    { title: '配置管理', path: '/system/config', icon: Setting },
    { title: '运行日志', path: '/system?tab=logs', icon: Document },
  ],
}
