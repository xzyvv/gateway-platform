<template>
  <el-card shadow="never" class="section-card">
    <template #header>
      <strong>{{ title }}（{{ problems.length }}）</strong>
    </template>
    <div v-if="!sorted.length" class="empty-ok">
      <el-icon><CircleCheck /></el-icon>
      <span>未发现需要处置的问题</span>
    </div>
    <div v-for="(p, i) in sorted" :key="`${p.code}-${i}`" class="problem">
      <el-tag :type="levelMeta(p.level).type" size="small" effect="dark" class="p-level">
        {{ levelMeta(p.level).label }}
      </el-tag>
      <div class="p-body">
        <div class="p-msg"><code class="p-code">{{ p.code }}</code>{{ p.message }}</div>
        <div class="p-hint">{{ p.hint }}</div>
      </div>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 分级问题清单（B1 共享展示）
 *
 * 数据只来自 `/api/health/report` 的 `problems`。健康报告、自诊断、网关信息
 * 三个页面共用这一个组件，保证同一份问题在不同页面的分级顺序、文案与配色一致，
 * 也避免三个页面各写一份排序/徽章映射后互相漂移。
 */
import { computed } from 'vue'
import { CircleCheck } from '@element-plus/icons-vue'
import { ElTag } from 'element-plus'
import type { HealthProblem } from '@/api/status'

const props = withDefaults(defineProps<{ problems: HealthProblem[]; title?: string }>(), {
  title: '待处置问题',
})

const LEVEL_ORDER: Record<string, number> = { critical: 0, warning: 1, info: 2 }

const sorted = computed(() =>
  [...props.problems].sort((a, b) => (LEVEL_ORDER[a.level] ?? 9) - (LEVEL_ORDER[b.level] ?? 9)),
)

const levelMeta = (level: string) => {
  if (level === 'critical') return { label: '严重', type: 'danger' as const }
  if (level === 'warning') return { label: '警告', type: 'warning' as const }
  return { label: '提示', type: 'info' as const }
}
</script>

<style scoped>
.section-card { margin-bottom: 16px; }
.empty-ok { display: flex; align-items: center; gap: 8px; color: var(--text-success); }
.problem { display: flex; gap: 10px; padding: 10px 0; border-bottom: 1px solid var(--el-border-color-lighter); }
.problem:last-child { border-bottom: none; }
.p-level { flex-shrink: 0; }
.p-msg { font-size: 0.9rem; }
.p-code { margin-right: 8px; color: var(--text-secondary); }
.p-hint { color: var(--text-secondary); font-size: 0.8rem; margin-top: 4px; }
</style>
