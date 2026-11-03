<template>
  <!-- 「常用优先」：首屏直出「数据查询」「传输与压缩」「质量与治理」7 项。 -->
  <HubShell
    :tabs="tabs"
    :group-labels="groupLabels"
    fallback-tab="history"
  />
</template>

<script setup lang="ts">
/**
 * 数据流水 Hub：历史数据 / 数据对比 / 数据压缩 / 断连续传 / 数据链路追踪 / 质量概览 / 采集退避。
 * 原「数据质量」Hub 已并入本 Hub 的「质量与治理」分组：查询、传输、质量同属一条数据流水线，
 * 分两个 Hub 会让「查历史 → 看质量 → 处理退避」跨页跳转。
 *
 * 页签用横向标签栏必然换行溢出，且 el-tabs 的 lazy 是「首次激活后永久常驻」，
 * 点一遍就是多套查询/图表常驻。分组后左侧竖排不换行，切走即卸载。
 * 「快照对比」页已下线：与版本历史页的对比弹窗同源（同一对端点、同一渲染逻辑）。
 * 「导出格式」页已下线：导出选择并入历史数据页工具条，且假的 xlsx 选项不再提供。
 * 「数据插值」页已下线（W1）：插值结果只在浏览器内计算、不写回网关历史，现场拿不到
 *   任何可留存或可上行的结论。
 * 「基线对比」页已下线（W1）：基准值/阈值只存在当前浏览器的 localStorage，网关不加载
 *   也不执行，换一台电脑或网关重装即丢失，无法承担「与昨日基线对比」这个真实场景。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const History = defineAsyncComponent({ loader: () => import('./pipeline/History.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DataCompare = defineAsyncComponent({ loader: () => import('./pipeline/DataCompare.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DataCompression = defineAsyncComponent({ loader: () => import('./pipeline/DataCompression.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DataBuffering = defineAsyncComponent({ loader: () => import('./pipeline/DataBuffering.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DataFlowTrace = defineAsyncComponent({ loader: () => import('./pipeline/DataFlowTrace.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DataQuality = defineAsyncComponent({ loader: () => import('./quality/DataQuality.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const BackoffMonitor = defineAsyncComponent({ loader: () => import('./quality/BackoffMonitor.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

// 分组沿用声明顺序，左侧导航即为 查询 → 传输 → 质量 的数据生命周期顺序
const groupLabels = { query: '数据查询', transfer: '传输与压缩', quality: '质量与治理' }

const tabs: HubTab[] = [
  { name: 'history', label: '历史数据', group: 'query', component: History, subtitle: '按点位与时间范围查询，支持降采样与聚合' },
  { name: 'data-compare', label: '区间对比', group: 'query', component: DataCompare, subtitle: '同一时间段内最多 4 个点位的数据对比' },
  { name: 'data-compression', label: '数据压缩', group: 'transfer', component: DataCompression, subtitle: '按设备对比压缩前后的采集点数与压缩率，配置死区阈值和变化上传' },
  { name: 'data-buffering', label: '断连续传', group: 'transfer', component: DataBuffering, subtitle: '断网期间的队列积压、内存占用与在线设备（缓存行为由网关内核决定，本页只做观测）' },
  { name: 'data-flow-trace', label: '链路追踪', group: 'transfer', component: DataFlowTrace, subtitle: '追踪单条数据从采集到上传的完整路径' },
  // 原「数据质量」Hub 的两个页签（原「质量分布」已并入通信画像，「数据清洗」已迁入协议转换）
  { name: 'data-quality', label: '质量概览', group: 'quality', component: DataQuality, subtitle: '质量等级分布、规则配置与点位实时状态' },
  { name: 'backoff-monitor', label: '采集退避', group: 'quality', component: BackoffMonitor, subtitle: '因通信失败自动退避的点位与重试节奏' },
]
</script>