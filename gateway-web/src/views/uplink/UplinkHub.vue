<template>
  <HubShell :tabs="tabs" :group-labels="groupLabels" fallback-tab="forward-config" />
</template>

<script setup lang="ts">
/** 上行转发 Hub：通道转发 + 上送模板、上行状态、透传会话、上行测试。
 *
 *  J 组收敛（W2-#13）：原「上行通道」「上行通道参数」「上行服务状态」三页都读同一份
 *  上行配置与同一份连接状态，用户分不清差异。现合并为两页：
 *   - 上行配置（forward-config，tab 名保留以兼容旧链接）= 通道转发（目标/周期/启停/测试）+ 上送模板（内容/编码）；
 *   - 上行状态（north-status）= 北向健康统一视图（通道连接/积压/成败 + 对外 Server 存活 + 透传会话）。
 *
 *  C3 术语收口：两个曾用同一前缀「上行通道」的入口拆分为
 *   - 「通道转发」= 数据发往哪里；
 *   - 「上送模板」= 数据长什么样。
 *  「北向」保留为搜索别名；对外提供数据的 Server（OPC UA / Modbus / 透传）统一称「对外服务 Server」。 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const UplinkConfigPage = defineAsyncComponent({ loader: () => import('./UplinkConfigPage.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const UplinkNorthStatus = defineAsyncComponent({ loader: () => import('./UplinkNorthStatus.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const UplinkPassthrough = defineAsyncComponent({ loader: () => import('./UplinkPassthrough.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const UplinkTester = defineAsyncComponent({ loader: () => import('./UplinkTester.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const groupLabels = { config: '通道配置', observe: '运行观测', debug: '调试工具' }

const tabs: HubTab[] = [
  { name: 'forward-config', label: '上行配置', group: 'config', component: UplinkConfigPage, subtitle: '「通道转发」配置数据发往哪里（目标/周期/启停/连接测试），「上送模板」配置数据长什么样（内容/编码/字节上限），配好后再到「上行状态」看连接结果' },
  { name: 'north-status', label: '上行状态', group: 'observe', component: UplinkNorthStatus, subtitle: '北向健康统一视图：一句话结论 + 上行通道连接数/消息积压/成功失败统计 + 对外服务 Server（OPC UA / Modbus / 透传）存活与透传会话' },
  { name: 'passthrough-sessions', label: '透传会话', group: 'debug', component: UplinkPassthrough, subtitle: '列出透传会话的总线、开始时间与超时时长' },
  { name: 'uplink-tester', label: '上行测试', group: 'debug', component: UplinkTester, subtitle: '编辑模板与测试数据预览渲染结果，可仅渲染或真实上行测试并保留调试历史' },
]
</script>
