<template>
  <div class="section-toggle">
    <el-switch
      :model-value="webhook !== null"
      @update:model-value="(val: boolean) => (webhook = val ? { enabled: true, url: '', headers: {}, timeout_secs: 10, batch_size: 100, interval_ms: 1000, max_retries: 3 } : null)"
      active-text="启用 Webhook"
      inactive-text="禁用"
    />
  </div>
  <el-form v-if="webhook" :model="webhook" label-width="140px" class="webhook-form">
    <el-form-item label="启用 Webhook">
      <el-switch v-model="webhook.enabled" />
    </el-form-item>
    <el-form-item label="目标 URL">
      <el-input v-model="webhook!.url" placeholder="https://api.example.com/gateway">
        <template #prefix><el-icon><Promotion /></el-icon></template>
      </el-input>
    </el-form-item>
    <el-form-item label="超时（秒）">
      <el-input-number v-model="webhook!.timeout_secs" :min="1" :max="60" />
    </el-form-item>
    <el-form-item label="批量大小">
      <!-- 批量上报按百条为单位调整，逐条微调没有实际意义 -->
      <el-input-number v-model="webhook!.batch_size" :min="1" :max="10000" :step="100" />
    </el-form-item>
    <el-form-item label="上报间隔">
      <!-- 后端以毫秒存储；DurationSelect 统一用秒，这里换算并允许毫秒档 -->
      <DurationSelect
        v-model="intervalSec"
        unit="ms"
        :units="['ms', 's', 'min']"
        :min="0.1"
        :max="3600"
        num-width="120px"
      />
    </el-form-item>
    <el-form-item label="重试次数">
      <el-input-number v-model="webhook!.max_retries" :min="0" :max="10" />
    </el-form-item>
    <el-form-item label="自定义请求头">
      <el-input :model-value="JSON.stringify(webhook!.headers || {}, null, 2)" @update:model-value="(val: string) => { try { webhook!.headers = JSON.parse(val || '{}') } catch { /* 忽略解析错误 */ } }" type="textarea" :rows="4" placeholder='{"Authorization": "Bearer xxx"}' />
    </el-form-item>
  </el-form>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import DurationSelect from '@/components/industrial/DurationSelect.vue'
import type { WebhookConfig } from '@/api/types'

const webhook = defineModel<WebhookConfig | null>({ required: true })

/** 上报间隔：后端字段是毫秒，DurationSelect 以秒为单位，读写时换算 */
const intervalSec = computed<number>({
  get: () => (webhook.value?.interval_ms ?? 1000) / 1000,
  set: (secs) => {
    if (!webhook.value) return
    webhook.value.interval_ms = Math.max(100, Math.round(secs * 1000))
  },
})
</script>