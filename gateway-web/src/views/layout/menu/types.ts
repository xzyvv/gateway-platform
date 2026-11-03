import type { Component } from 'vue'

export interface MenuItem {
  title: string
  path: string
  icon: Component
  /** 悬停提示：写清与相近入口的边界（如「报文调试」构造下发 vs「总线抓包」观察原始帧） */
  description?: string
}

export interface MenuGroup {
  name: string
  icon: Component
  items: MenuItem[]
}

export interface NavIndexItem {
  id: string
  title: string
  subtitle?: string
  /** 别名词典：搜索主名与菜单/页签实际文字不一致时在此登记，
   *  让侧栏搜索按用户看到的字也能命中（守护测试见 navTargets.spec.ts）。 */
  alias?: string
  icon: Component
  /** 站内目标：裸路径打开 Hub 默认页签，`?tab=` 打开指定页签 */
  target: string
}
