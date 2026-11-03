/**
 * menu 模块聚合出口：
 * - 类型定义见 ./types
 * - 菜单组按业务域拆分到 ./groups/*，新增菜单组请按业务域放置并在下方导入
 * - 侧栏搜索索引（页面 + Hub 页签深链）见 ./navIndex
 *
 * 用法（替代原 menuGroups）：
 *   import { useFilteredMenuGroups, navIndexItems } from './layout/menu'
 *
 * menuGroups 现在保留原始未过滤版本，仅给脚本生成 / 文档使用；
 * 页面渲染请用 useFilteredMenuGroups() 返回的 computed。
 */
import { computed } from 'vue'
import type { MenuGroup, MenuItem, NavIndexItem } from './types'
import { overviewGroup } from './groups/overview'
import { devicesGroup } from './groups/devices'
import { pointsGroup } from './groups/points'
import { acquisitionGroup } from './groups/acquisition'
import { dataGroup } from './groups/data'
import { uplinkGroup } from './groups/uplink'
import { opsGroup } from './groups/ops'
import { systemGroup } from './groups/system'
import { navIndexItems as _navIndex } from './navIndex'

export type { MenuItem, MenuGroup, NavIndexItem }

/** 原始菜单组。脚本生成 / 文档 / E2E 用。 */
export const menuGroups: MenuGroup[] = [
  overviewGroup,
  devicesGroup,
  pointsGroup,
  acquisitionGroup,
  dataGroup,
  uplinkGroup,
  opsGroup,
  systemGroup,
]

/**
 * 按路由路径解析所属菜单分组名。
 * 侧栏展开态与 Layout 面包屑共用此单一来源，避免各自重写匹配规则。
 *
 * 匹配规则：path.startsWith(菜单项 path)。
 * 约定菜单项 path 与路由域一致——路由归属与菜单分组不一致的入口应把路由迁到对应域
 * （如数据加密原先在 /data/encryption，却归「系统」组，已迁至 /system/encryption）。
 */
export function findGroupNameByPath(path: string): string {
  const g = menuGroups.find((grp) => grp.items.some((i) => path.startsWith(i.path)))
  return g ? g.name : ''
}

/**
 * 返回指定分组的首个菜单项路径（面包屑「分组」层可点击目标）。
 * 找不到分组时返回空串。
 */
export function findGroupFirstPath(groupName: string): string {
  const g = menuGroups.find((grp) => grp.name === groupName)
  return g?.items[0]?.path ?? ''
}

/**
 * 菜单组（v2.0 全档位可见，不做过滤）。
 *
 * Vue usage:
 *   const filtered = useFilteredMenuGroups()
 *   <el-sub-menu v-for="g in filtered" :key="g.name">...
 *
 * 保留 computed 包装与函数名以兼容既有调用方，未来可直接内联。
 */
export function useFilteredMenuGroups() {
  return computed(() => menuGroups)
}

/** 侧栏搜索索引（页面 + Hub 页签深链），v2.0 全量返回。 */
export function useNavIndex() {
  return computed(() => _navIndex as NavIndexItem[])
}

/** 搜索索引（全部）。侧栏搜索消费此表。 */
export const navIndexItems = _navIndex
