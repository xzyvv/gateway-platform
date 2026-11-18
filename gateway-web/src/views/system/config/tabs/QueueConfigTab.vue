<template>
  <div v-if="queue" class="queue-tab">
    <el-form :model="queue" label-width="140px" class="queue-form">
      <el-form-item label="队列行数上限">
        <PresetNumberSelect
          v-model="queue.max_rows"
          :options="QUEUE_ROW_PRESETS"
          :min="200000"
          :max="50000000"
          :format="formatRows"
          aria-label="队列行数上限"
        />
        <div class="field-hint">
          原始数据队列最多保留的行数（20 万 ~ 5000 万，默认 100 万）；
          按单行约 200B 估算磁盘占用，超出上限后最旧数据先被淘汰。
        </div>
      </el-form-item>

      <!-- 容量估算器：按现场点位数/周期/缓冲时长反推建议值，消除默认 100 万行不够用的误判 -->
      <el-form-item label="容量估算">
        <div class="capacity-estimator">
          <div class="estimate-inputs">
            <el-input-number v-model="pointCount" :min="0" :max="100000" :step="100" size="small" aria-label="点位数" />
            <span class="estimate-unit">点位</span>
            <el-input-number v-model="intervalSec" :min="1" :max="3600" :step="5" size="small" aria-label="采集周期秒" />
            <span class="estimate-unit">秒/次</span>
            <el-select v-model="bufferHours" size="small" aria-label="断网缓冲时长" class="buffer-select">
              <el-option v-for="h in BUFFER_HOURS" :key="h" :label="`${h} 小时`" :value="h" />
            </el-select>
          </div>

          <el-descriptions :column="2" border size="small" class="estimate-result">
            <el-descriptions-item label="建议 max_rows">
              {{ formatRows(estimate.suggestedMaxRows) }}
              <el-tag v-if="isBelowSuggestion" type="warning" size="small" class="hint-tag">当前偏低</el-tag>
              <el-button v-if="isBelowSuggestion" link type="primary" size="small" @click="applySuggestion">应用</el-button>
            </el-descriptions-item>
            <el-descriptions-item label="建议卡容量">{{ formatBytes(estimate.suggestedCardBytes) }}</el-descriptions-item>
            <el-descriptions-item label="每天写入">{{ formatRows(estimate.rowsPerDay) }}</el-descriptions-item>
            <el-descriptions-item label="原始库+备份">
              {{ formatBytes(estimate.rawBytes) }} × 2
            </el-descriptions-item>
          </el-descriptions>

          <!-- #1：聚合表覆盖天数显性化，纠偏「卡越大历史越长」的误判 -->
          <el-alert type="info" :closable="false" show-icon class="coverage-alert">
            <template #title>
              聚合表（分钟桶）在此规模下约覆盖 <strong>{{ coverageDays.toFixed(1) }} 天</strong>
            </template>
            聚合表行数上限 = 磁盘 2% 预算，每天桶数随点数线性增长——主要由点位数决定，
            不是磁盘越大历史越长。要留长历史请调大 max_rows 并定期导出。
          </el-alert>
        </div>
      </el-form-item>
      <el-alert type="warning" show-icon :closable="false" title="修改后需重启网关">
        淘汰线在存储层初始化时固定，需重启才完整生效；
        积压/背压/容量压力的水位判定会立即按新值重新计算。
      </el-alert>
    </el-form>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import PresetNumberSelect from '@/components/industrial/PresetNumberSelect.vue'
import type { GatewayConfig } from '@/api/types'
import { estimateCapacity, aggregateCoverageDays } from '@/utils/capacityPlan'
import { formatBytes } from '@/utils/format'

const queue = defineModel<GatewayConfig['queue']>({ required: true })

/**
 * 现场规模：宿主传入真实点位数/周期作为初始值，估算器内部可再调整。
 * 用 props + 本地 ref 而非 defineModel——这些值只在估算器内编辑，不需回写宿主。
 */
const props = withDefaults(
  defineProps<{ pointCount?: number; intervalSec?: number; bufferHours?: number }>(),
  { pointCount: 500, intervalSec: 10, bufferHours: 24 },
)
const pointCount = ref(props.pointCount)
const intervalSec = ref(props.intervalSec)
const bufferHours = ref(props.bufferHours)
// 宿主数据加载完成后（usedPoints 从 null 变为真实值）同步一次初始值
watch(() => props.pointCount, (v) => { pointCount.value = v })
watch(() => props.intervalSec, (v) => { intervalSec.value = v })

/** 常用档位（20 万 ~ 5000 万），仍可直接输入区间内任意整数 */
const QUEUE_ROW_PRESETS = [
  200_000, 500_000, 1_000_000, 2_000_000, 5_000_000, 10_000_000, 50_000_000,
]
const BUFFER_HOURS = [4, 8, 24, 72]

/** 大数字按「万」展示，避免用户数 0 数错 */
const formatRows = (n: number) =>
  `${(n / 10_000).toLocaleString('zh-CN', { maximumFractionDigits: 2 })} 万行`

const estimate = computed(() =>
  estimateCapacity({
    pointCount: pointCount.value,
    intervalSec: intervalSec.value,
    bufferHours: bufferHours.value,
  }),
)

/** 聚合表覆盖天数用建议卡容量估算（尚未落盘时的预判）。 */
const coverageDays = computed(() =>
  aggregateCoverageDays(estimate.value.suggestedCardBytes, pointCount.value),
)

/** 当前配置低于建议值时提示应用，避免改动用户已有的更大配置。 */
const isBelowSuggestion = computed(() => queue.value.max_rows < estimate.value.suggestedMaxRows)

function applySuggestion() {
  queue.value.max_rows = estimate.value.suggestedMaxRows
}
</script>

<style scoped>
.queue-form {
  max-width: 720px;
}
.field-hint {
  margin-top: 4px;
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--text-secondary);
}
.capacity-estimator {
  width: 100%;
}
.estimate-inputs {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-bottom: 10px;
  flex-wrap: wrap;
}
.estimate-unit {
  font-size: 0.8rem;
  color: var(--text-secondary);
}
.buffer-select {
  width: 120px;
}
.estimate-result {
  margin-bottom: 10px;
}
.hint-tag {
  margin-left: 6px;
}
.coverage-alert {
  line-height: 1.5;
}
</style>
