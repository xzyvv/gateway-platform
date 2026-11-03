<template>
  <HubShell :tabs="tabs" :group-labels="groupLabels" fallback-tab="devices" />
</template>

<script setup lang="ts">
/** 设备 Hub：日常维护（设备清单 / 分组 / 批量命令）、运行评估与干预（健康评分 / 远程控制）、
 *  建站建档（设备模板 / 批量导入 / 设备发现）。
 *  原「设备运维」Hub（3 页签）已并入本 Hub：同属设备域，只有建站与运维两种使用场景，
 *  分两个 Hub 会让「看健康分 → 去远程控制」这类连续动作跨页跳转。
 *  「资产台账」页已并入设备清单（A7）：资产信息按设备维度在设备行打开，不再单独占页签。
 *  R3：「设备状态快照」不再单列页签，并入健康评分页作为「期望 vs 实测」子表——
 *  两者同看设备运行状态，分开会让用户在两页之间反复比对评分与快照。 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const Devices = defineAsyncComponent({ loader: () => import('./DeviceList.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DeviceGroups = defineAsyncComponent({ loader: () => import('./DeviceGroups.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const BatchCommand = defineAsyncComponent({ loader: () => import('./DeviceBatchCommand.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const HealthScore = defineAsyncComponent({ loader: () => import('./HealthScore.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const RemoteControl = defineAsyncComponent({ loader: () => import('./RemoteControl.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DeviceTemplates = defineAsyncComponent({ loader: () => import('./DeviceTemplates.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const BatchImport = defineAsyncComponent({ loader: () => import('./DeviceBatchImport.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DiscoverWizard = defineAsyncComponent({ loader: () => import('./DeviceDiscovery.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

// 分组：日常维护（已有设备）→ 运行评估（只读，含期望 vs 实测）→ 运行干预（写设备）→ 建站与建档（新设备进场）
const groupLabels = { manage: '日常维护', assess: '运行评估', intervene: '运行干预', provision: '建站与建档' }

const tabs: HubTab[] = [
  { name: 'devices', label: '设备清单', group: 'manage', component: Devices, subtitle: '设备的新增、编辑、删除与连通性检测', pin: true },
  { name: 'device-groups', label: '设备分组', group: 'manage', component: DeviceGroups, subtitle: '新建分组、设置颜色并为分组选择设备' },
  { name: 'batch-command', label: '批量命令', group: 'manage', component: BatchCommand, subtitle: '按点位批量读取或写入，确认后向所有选中点位真实下发并查看各目标结果与历史' },
  { name: 'health-score', label: '设备健康评分', group: 'assess', component: HealthScore, subtitle: '设备平均健康分与等级分布，查看单台成功率、延迟与趋势详情；页内附「期望 vs 实测」状态快照' },
  { name: 'remote-control', label: '远程控制', group: 'intervene', component: RemoteControl, subtitle: '对点位下发写值，并查看下行执行历史' },
  { name: 'device-templates', label: '设备模板', group: 'provision', component: DeviceTemplates, subtitle: '保存总线、协议与点位配置，可应用到新设备' },
  { name: 'batch-import', label: '批量导入', group: 'provision', component: BatchImport, subtitle: '可批量导入设备或点位：下载 CSV 模板上传校验（点位另支持粘贴 JSON），确认后批量写入网关配置' },
  { name: 'discover-wizard', label: '设备发现', group: 'provision', component: DiscoverWizard, subtitle: '按总线与扫描范围查找设备，批量导入设备清单' },
]
</script>
