<template>
  <HubShell :tabs="tabs" fallback-tab="tester" />
</template>

<script setup lang="ts">
/** 读写测试 Hub：点位读写（单点读写 + 批量写入）、读取性能（单点位连续读取压测）。
 *  收敛协议工具组中「读写测试 / 批量读写测试」两个平铺入口。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const Tester = defineAsyncComponent({ loader: () => import('./simulate/Tester.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const BatchReadTest = defineAsyncComponent({ loader: () => import('./simulate/BatchReadTest.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const tabs: HubTab[] = [
  { name: 'tester', label: '点位读写', component: Tester },
  { name: 'batch-read-test', label: '读取性能', component: BatchReadTest },
]
</script>
