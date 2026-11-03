<template>
  <div class="section-toggle">
    <el-switch
      :model-value="telemetry !== null"
      @update:model-value="(val: boolean) => (telemetry = val ? { enabled: true, otlp_endpoint: 'http://localhost:4317', service_name: 'gateway', sample_ratio: 1.0, metrics_interval_secs: 15 } : null)"
      active-text="启用 OTLP"
      inactive-text="禁用"
    />
  </div>
  <el-form v-if="telemetry" :model="telemetry" label-width="160px" class="telemetry-form">
    <el-form-item label="启用 OTLP 导出">
      <el-switch v-model="telemetry.enabled" />
    </el-form-item>
    <el-form-item label="OTLP gRPC 端点">
      <el-input v-model="telemetry!.otlp_endpoint" placeholder="http://collector:4317" />
    </el-form-item>
    <el-form-item label="服务名">
      <el-input v-model="telemetry!.service_name" placeholder="gateway-01" />
    </el-form-item>
    <el-form-item label="采样率">
      <el-slider v-model="telemetry!.sample_ratio" :min="0" :max="1" :step="0.1" show-input :marks="{ 0: '0%', 0.5: '50%', 1: '100%' }" />
    </el-form-item>
    <el-form-item label="指标间隔">
      <DurationSelect v-model="telemetry!.metrics_interval_secs" :min="5" :max="300" :units="['s']" />
    </el-form-item>
  </el-form>
</template>

<script setup lang="ts">
import DurationSelect from '@/components/industrial/DurationSelect.vue'
import type { GatewayConfig } from '@/api/types'

// TelemetryConfig 未从 @/api/types 导出（ts-rs 生成的 index 未含该类型），
// 故从 GatewayConfig 的 telemetry 节推导其类型，避免深度引入 generated 文件。
type TelemetryConfig = NonNullable<GatewayConfig['telemetry']>

const telemetry = defineModel<TelemetryConfig | null>({ required: true })
</script>