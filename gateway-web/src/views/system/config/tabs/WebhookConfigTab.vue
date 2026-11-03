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
      <el-input-number v-model="webhook!.batch_size" :min="1" :max="10000" />
    </el-form-item>
    <el-form-item label="上报间隔">
      <IntervalSelect v-model="webhook!.interval_ms" />
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
import IntervalSelect from '@/components/industrial/IntervalSelect.vue'
import type { WebhookConfig } from '@/api/types'

const webhook = defineModel<WebhookConfig | null>({ required: true })
</script>