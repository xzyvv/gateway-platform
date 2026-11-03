<template>
  <el-timeline v-if="steps.length" aria-live="polite">
    <el-timeline-item v-for="s in steps" :key="s.id" :type="s.status === 'success' ? 'success' : s.status === 'fail' ? 'danger' : 'primary'">
      <div class="step">
        <strong>{{ s.param }}</strong>
        <el-tag v-if="s.status === 'success'" type="success" size="small" effect="dark" round style="margin-left:8px">{{ s.value }}</el-tag>
        <span v-if="s.status === 'testing'" style="margin-left:8px;color:var(--el-color-primary)">探测中...</span>
      </div>
      <!-- 审查#6：失败步骤的 detail 就地展示并拼诊断建议，不再只有全局 toast 看得到提示 -->
      <div v-if="s.status === 'fail' && s.detail" class="fail-detail">{{ withDiagHint(s.detail) }}</div>
    </el-timeline-item>
  </el-timeline>
</template>

<script setup lang="ts">
/**
 * 参数探测「逐项进度」时间线（从 AutoDetect.vue 拆出）
 *
 * 职责：按探测顺序展示每个参数项的当前状态——成功显示值、探测中显示提示、
 * 失败时就地展开原因并拼诊断建议（不再只有一闪而过的全局 toast）。
 *
 * 纯展示区块：数据由 useAutoDetect 的轮询结果驱动，组件本身不发请求。
 *
 * Props:
 * - steps：探测项列表（含 pending/testing/success/fail 状态）
 */
import type { DetectStep } from '@/api/protocolTools'
import { withDiagHint } from '@/utils/diagHints'

defineProps<{
  /** 探测项列表 */
  steps: DetectStep[]
}>()
</script>

<style scoped>
.step { display: flex; align-items: center; }
.fail-detail { margin-top: 4px; color: var(--text-danger); font-size: 0.8125rem; }
</style>
