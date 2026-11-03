<template>
  <HubShell :tabs="tabs" :group-labels="groupLabels" fallback-tab="comm-params" />
</template>

<script setup lang="ts">
/** 采集调优 Hub（原「通信优化」，采集域）：通信参数、通信画像、调度优化。
 *  与「链路诊断」（抓包/连通/异常码）的边界：本 Hub 是参数与调度优化，不做现场诊断。
 *  点位模板已并入「设备模板」页（同一后端 /api/templates 实体），不再单独成页。
 *  N-5：原「通信矩阵」页签与「通信画像」同一数据源（stats-report），已合并为
 *  通信画像页内的「表格 / 矩阵」形态切换，不再占用独立页签。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const CommParams = defineAsyncComponent({ loader: () => import('./CommParams.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const CommProfile = defineAsyncComponent({ loader: () => import('./CommProfile.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ScheduleOptimize = defineAsyncComponent({ loader: () => import('./ScheduleOptimize.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const groupLabels = { runtime: '链路参数', optimize: '调优分析' }

const tabs: HubTab[] = [
  { name: 'comm-params', label: '通信参数', group: 'runtime', component: CommParams, subtitle: '按设备编辑字节序，超时与重试为总线级参数，改动影响同总线全部设备' },
  { name: 'comm-profile', label: '通信画像', group: 'optimize', component: CommProfile, subtitle: '各设备的响应时间、成功率、丢包率与优化建议，支持表格与通信矩阵两种形态' },
  { name: 'schedule-optimize', label: '调度优化', group: 'optimize', component: ScheduleOptimize, subtitle: '合并相邻地址段（间隔不超过 5）的点位读取，给出优化率与合并组' },
]
</script>