<template>
  <HubShell
    :tabs="tabs"
    :group-labels="groupLabels"
    fallback-tab="config-versions"
  />
</template>

<script setup lang="ts">
/** 配置管理 Hub：按「配置 / 运维」两类语义归组（与 SystemHub 统一口径）。
 *  - 配置（config）：部署向导、运行参数（页内底部可折叠内嵌参数同步）、导入导出、片段导入；
 *  - 运维（ops）：版本历史（内含对比与撤销）、配置体检。
 *  B5：原独立「参数同步」页签已并入「运行参数」（同步的就是该页编辑的总线超时与重试参数，
 *  拆两个页签会让「先看参数再回上页改」多一次跳转）。
 *  分组后左侧竖排导航不再换行，且只有当前 tab 挂载（见 HubShell 说明）。
 *  「版本对比」「撤销重做」两个独立页签已并入版本历史页（对比弹窗 + 撤销到上一版本按钮）。
 *  「配置锁定」页已下线：其锁定状态只存 localStorage，后端无对应端点、全仓无消费者（F-10）。
 *  「网络配置」页已下线：与「健康检查 › 自诊断」同源（同一 /api/diagnostics/gateway 的子集），
 *  收/发统计已并入自诊断页的网络接口表。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const RuntimeConfig = defineAsyncComponent({ loader: () => import('./RuntimeConfig.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const Wizard = defineAsyncComponent({ loader: () => import('./Wizard.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ConfigVersions = defineAsyncComponent({ loader: () => import('./ConfigVersions.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ConfigHealth = defineAsyncComponent({ loader: () => import('./ConfigHealth.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ConfigIo = defineAsyncComponent({ loader: () => import('./ConfigIo.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ConfigFragmentImport = defineAsyncComponent({ loader: () => import('./ConfigFragmentImport.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const groupLabels = { config: '配置', ops: '运维' }

const tabs: HubTab[] = [
  { name: 'wizard', label: '部署向导', group: 'config', component: Wizard, subtitle: '按真实链路配置总线 → 设备 → 点位 → 上行 → 采集验证', pin: true },
  { name: 'runtime-config', label: '运行参数', group: 'config', component: RuntimeConfig, subtitle: '按总线、Web、MQTT 等分节编辑并热重载；页内底部可展开「参数同步」在总线间同步超时与重试参数', pin: true },
  { name: 'config-versions', label: '版本历史', group: 'ops', component: ConfigVersions, subtitle: '查看配置变更历史、回滚到任意版本', pin: true },
  { name: 'config-health', label: '配置体检', group: 'ops', component: ConfigHealth, subtitle: '扫描配置中的矛盾项与高风险写法' },
  { name: 'config-io', label: '导入导出', group: 'config', component: ConfigIo, subtitle: '导入导出完整配置，以及总线、上行通道、变量映射工程文件' },
  { name: 'fragment-import', label: '片段导入', group: 'config', component: ConfigFragmentImport, subtitle: '粘贴或上传配置片段，按主键合并进当前配置，其余节原样保留' },
]
</script>