<template>
  <HubShell :tabs="tabs" :group-labels="groupLabels" fallback-tab="points" />
</template>

<script setup lang="ts">
/** 点位管理 Hub：点位管理、虚拟点位、点位克隆、查找替换、地址工具。
 *  收敛点位组中全部平铺入口（v2.0 起全部页签对所有档位开放）。
 *  原「地址换算」Hub（地址换算 / 地址偏移两页签）已并入本 Hub 的「进阶工具」分组：
 *  两者都是按点位地址换算的工具页，独立成 Hub 只是多一层菜单与路由。
 *  原「导入预校验」已改为「配置管理 → 片段导入」，归到配置域。
 *  原「CSV 批量导入」已并入「设备管理 → 批量导入」（同一流程可选设备或点位）。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const Points = defineAsyncComponent({ loader: () => import('./PointList.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const PointsTrash = defineAsyncComponent({ loader: () => import('./PointTrash.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const FormulaPoints = defineAsyncComponent({ loader: () => import('./PointFormula.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const PointClone = defineAsyncComponent({ loader: () => import('./PointClone.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const FindReplace = defineAsyncComponent({ loader: () => import('./PointFindReplace.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const AddrCalculator = defineAsyncComponent({ loader: () => import('./AddrCalculator.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const AddressOffset = defineAsyncComponent({ loader: () => import('./AddressOffset.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const groupLabels = { manage: '点位维护', batch: '批量操作', advanced: '进阶工具' }

const tabs: HubTab[] = [
  { name: 'points', label: '点位清单', group: 'manage', component: Points, subtitle: '单点增删改查、批量操作与允许写入设置' },
  { name: 'point-trash', label: '已删除点位', group: 'manage', component: PointsTrash, subtitle: '已删除点位可恢复或彻底删除（保留 30 天）' },
  { name: 'formula-points', label: '虚拟点位', group: 'manage', component: FormulaPoints, subtitle: '按公式引用其它点位计算，支持公式校验与重算' },
  { name: 'point-clone', label: '点位克隆', group: 'batch', component: PointClone, subtitle: '按现有点位批量复制生成同类点位' },
  { name: 'find-replace', label: '查找替换', group: 'advanced', component: FindReplace, subtitle: '按名称、地址或单位查找匹配项后批量替换' },
  // 原「地址换算」Hub 的两个页签：都是按地址做换算/偏移的单点工具，归入进阶工具
  { name: 'addr-calculator', label: '地址换算', group: 'advanced', component: AddrCalculator, subtitle: '按 Modbus 地址与功能码换算系统地址，并输出请求报文' },
  { name: 'address-offset', label: '地址偏移', group: 'advanced', component: AddressOffset, subtitle: '对选中点位批量应用地址偏移' },
]
</script>
