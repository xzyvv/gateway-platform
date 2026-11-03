<template>
  <div class="ct-summary" v-if="summary.total > 0">
    <div class="ct-stat">
      <div class="ct-stat-label">测试项</div>
      <div class="ct-stat-value">{{ summary.total }}</div>
    </div>
    <div class="ct-stat">
      <div class="ct-stat-label">通过</div>
      <div class="ct-stat-value ok">{{ summary.pass }}</div>
    </div>
    <div class="ct-stat">
      <div class="ct-stat-label">失败</div>
      <div class="ct-stat-value bad">{{ summary.fail }}</div>
    </div>
    <div class="ct-stat">
      <div class="ct-stat-label">平均耗时</div>
      <div class="ct-stat-value">{{ summary.avgLatency }}ms</div>
    </div>
    <div class="ct-stat" v-if="summary.lastAt">
      <div class="ct-stat-label">最近测试</div>
      <div class="ct-stat-value" style="font-size: 0.875rem">{{ summary.lastAt }}</div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * TestSummaryPanel — 测试结果汇总面板（展示型组件）
 *
 * 由 ConnectionTest 容器传入 summary（total/pass/fail/avgLatency/lastAt），
 * total 为 0 时整体隐藏。无副作用、无 emit。
 */
import type { TestSummary } from './types'

defineProps<{
  summary: TestSummary
}>()
</script>

<style scoped>
.ct-summary {
  display: flex;
  gap: 24px;
  margin-top: 16px;
  padding-top: 16px;
  border-top: 1px solid var(--border-extralight);
}
.ct-stat { text-align: center; min-width: 70px; }
.ct-stat-label { color: var(--text-secondary); font-size: 0.75rem; margin-bottom: 4px; }
.ct-stat-value { font-size: 1.75rem; font-weight: 700; color: var(--text-primary); font-variant-numeric: tabular-nums; }
.ct-stat-value.ok { color: var(--text-success); }
.ct-stat-value.bad { color: var(--text-danger); }
</style>