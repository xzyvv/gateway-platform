<template>
  <el-alert :title="failedCount ? '探测完成，但部分参数失败' : '探测完成，可应用以下参数'" :type="failedCount ? 'warning' : 'success'" show-icon :closable="false" style="margin-top:12px" />
  <template v-if="successSteps.length">
    <el-descriptions :column="1" border size="small" class="params" aria-label="探测到的参数">
      <el-descriptions-item v-for="s in successSteps" :key="s.id" :label="s.param">{{ s.value }}</el-descriptions-item>
    </el-descriptions>
    <div class="actions">
      <el-button size="small" @click="emit('copy')">复制参数</el-button>
      <el-button size="small" type="primary" @click="emit('apply')">前往「通信参数」应用</el-button>
    </div>
  </template>
</template>

<script setup lang="ts">
/**
 * 参数探测「成功参数汇总」区块（从 AutoDetect.vue 拆出）
 *
 * 审查#8：补完「可应用以下参数」死胡同——成功参数汇总成表，
 * 可一键复制或跳转通信参数页应用，而不是探测成功后无处落地。
 *
 * 组件只负责展示与意图上报：复制内容由 useAutoDetect 组装，
 * 跳转路由由页面主组件处理。
 *
 * Props:
 * - successSteps：探测成功的参数项
 * - failedCount：失败项数量（决定顶部提示是 warning 还是 success）
 *
 * Emits:
 * - copy：复制参数
 * - apply：跳转「通信参数」页
 */
import type { DetectStep } from '@/api/protocolTools'

defineProps<{
  /** 探测成功的参数项 */
  successSteps: DetectStep[]
  /** 失败项数量 */
  failedCount: number
}>()

const emit = defineEmits<{
  /** 复制参数 */
  (e: 'copy'): void
  /** 跳转「通信参数」页 */
  (e: 'apply'): void
}>()
</script>

<style scoped>
.params { margin-top: 12px; }
.actions { margin-top: 12px; display: flex; gap: 8px; }
</style>
