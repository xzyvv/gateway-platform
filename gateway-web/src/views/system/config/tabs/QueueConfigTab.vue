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
      <el-alert type="warning" show-icon :closable="false" title="修改后需重启网关">
        淘汰线在存储层初始化时固定，需重启才完整生效；
        积压/背压/容量压力的水位判定会立即按新值重新计算。
      </el-alert>
    </el-form>
  </div>
</template>

<script setup lang="ts">
import PresetNumberSelect from '@/components/industrial/PresetNumberSelect.vue'
import type { GatewayConfig } from '@/api/types'

const queue = defineModel<GatewayConfig['queue']>({ required: true })

/** 常用档位（20 万 ~ 5000 万），仍可直接输入区间内任意整数 */
const QUEUE_ROW_PRESETS = [
  200_000, 500_000, 1_000_000, 2_000_000, 5_000_000, 10_000_000, 50_000_000,
]

/** 大数字按「万」展示，避免用户数 0 数错 */
const formatRows = (n: number) =>
  `${(n / 10_000).toLocaleString('zh-CN', { maximumFractionDigits: 2 })} 万行`
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
</style>
