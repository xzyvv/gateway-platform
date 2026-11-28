<template>
  <!-- keep-alive：两页都是「做完一轮还要对着结果复盘」的会话型页面。
       切页签时「点位读写」的操作记录（含请求/响应 HEX）与「读取性能」的 P50/P95/P99
       结果、测试日志都是现场证据，卸载即丢；缓存后切回还在原处。 -->
  <HubShell :tabs="tabs" fallback-tab="tester" keep-alive />
</template>

<script setup lang="ts">
/** 读写测试 Hub：点位读写（单点读写 + 批量写入）、读取性能（单点位连续读取压测）。
 *  收敛协议工具组中「读写测试 / 批量读写测试」两个平铺入口。
 *
 * ## 与「协议调试台」的分工
 * 调试台的目标条是 ConsoleTargetBar（**总线**维度），因为那边的动作（扫描/抓包/回放）
 * 都以总线为单位。本 Hub 的两个页面都以「设备 → 点位」为单位，各自在卡片里带
 * DevicePointSelector，并共用 `useSharedDebugTarget().selectedDeviceId`——
 * 在这里再挂一条按总线选的工具条会是一个对本页毫无作用、却看起来能改选目标的死控件，
 * 因此有意不传 #toolbar。
 *
 * ## 为什么不分 group
 * 分组导航的价值在于「把 9~13 个页签压成几行」。两个页签分两组会得到两个只含一项的
 * 分组标题，纯占高度；保持单分组（不显示分组标题）时左侧导航就是两行，语义已经清楚。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/views/layout/HubShell.vue'
import type { HubTab } from '@/views/layout/HubShell.vue'

const Tester = defineAsyncComponent({ loader: () => import('./simulate/Tester.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const BatchReadTest = defineAsyncComponent({ loader: () => import('./simulate/BatchReadTest.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const tabs: HubTab[] = [
  {
    name: 'tester',
    label: '点位读写',
    component: Tester,
    subtitle: '选设备与点位读当前值、下发写入或发起协议诊断；每次收发都记录原始报文，可导出为 CSV',
  },
  {
    name: 'batch-read-test',
    label: '读取性能',
    component: BatchReadTest,
    subtitle: '对同一个点位连续读取 N 次，统计 P50 / P95 / P99 与成功率，评估采集链路稳定性',
  },
]
</script>
