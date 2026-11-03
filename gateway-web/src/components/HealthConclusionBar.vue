<template>
  <el-card shadow="never" class="section-card hcb">
    <div class="hcb-row">
      <el-tag :type="status.type" effect="dark" size="small">{{ status.label }}</el-tag>
      <span class="hcb-count">严重 <b>{{ counts.critical }}</b></span>
      <span class="hcb-count">警告 <b>{{ counts.warning }}</b></span>
      <span class="hcb-count">提示 <b>{{ counts.info }}</b></span>
      <span v-if="summaryText" class="hcb-summary">{{ summaryText }}</span>
      <span v-if="generatedAt" class="hcb-generated">{{ generatedAt }}</span>
      <el-button class="hcb-link" type="primary" link size="small" @click="goReport">查看健康报告 →</el-button>
    </div>
  </el-card>
</template>

<script setup lang="ts">
/**
 * 健康结论条（R2：入口收敛）。
 *
 * 网关级健康结论此前分散在健康报告、自诊断、网关信息、交付自检四处各自渲染，
 * 同一台网关在同一时刻可能出现多个「结论」，现场不知道该信哪一个。
 *
 * 收敛规则：`运维 › 健康报告` 是唯一的结论页（完整汇总 + 逐总线明细 + 分级处置），
 * 其它页面只留这一行结论摘要，并统一深链到该页；问题清单仍复用 HealthProblemList。
 */
import { computed } from 'vue'
import { useRouter } from 'vue-router'
import type { HealthProblem } from '@/api/status'
import type { OverallHealthStatus, ProblemCounts } from '@/stores/healthReport'

const props = withDefaults(
  defineProps<{
    /** 后端 /api/health/report 的分级问题清单（与 HealthProblemList 同一份数据） */
    problems: HealthProblem[]
    /** 综合状态（正常 / 降级 / 严重 / 暂无数据） */
    status: OverallHealthStatus
    /** 一行摘要，如「3 条总线 · 12/14 台设备在线」 */
    summaryText?: string
    /** 报告生成时间戳（毫秒），无快照时不传 */
    generatedAtMs?: number
  }>(),
  { summaryText: '', generatedAtMs: 0 },
)

/** 计数在这里算一次，避免调用方各自 reduce 后与问题清单对不上 */
const counts = computed<ProblemCounts>(() => {
  const c: ProblemCounts = { critical: 0, warning: 0, info: 0 }
  for (const p of props.problems) {
    if (p.level === 'critical' || p.level === 'warning' || p.level === 'info') c[p.level] += 1
  }
  return c
})

const generatedAt = computed(() => {
  const ms = props.generatedAtMs
  if (!ms) return ''
  const d = new Date(ms)
  const pad = (n: number) => String(n).padStart(2, '0')
  return `生成于 ${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
})

const router = useRouter()

function goReport() {
  router.push({ path: '/ops', query: { tab: 'health-report' } })
}
</script>

<style scoped>
.section-card {
  margin-bottom: 16px;
}
.hcb-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 16px;
  font-size: 0.8125rem;
  color: var(--text-secondary);
}
.hcb-count b {
  color: var(--text-primary);
  margin-left: 2px;
}
.hcb-summary {
  margin-left: auto;
}
.hcb-generated {
  color: var(--text-placeholder);
  font-size: 0.78rem;
}
.hcb-link {
  flex-shrink: 0;
}
</style>
