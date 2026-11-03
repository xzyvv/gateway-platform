<template>
  <div>
    <el-form label-width="104px" style="max-width: 560px">
      <el-form-item label="启用 MQTT">
        <el-switch v-model="uplinkForm.enabled" />
      </el-form-item>
      <el-form-item label="Broker 地址" required>
        <el-input v-model="uplinkForm.broker" placeholder="mqtt://127.0.0.1:1883" />
        <div class="field-hint">格式 mqtt://主机:端口；TLS 场景写 mqtts://主机:8883 并在运行参数页配置证书。</div>
      </el-form-item>
      <el-form-item label="上报主题" required>
        <el-input v-model="uplinkForm.topic" placeholder="如：gateway/data" />
      </el-form-item>
      <el-form-item label="上传周期">
        <el-input-number v-model="uplinkForm.publishInterval" :min="1" :max="3600" controls-position="right" />
        <span class="unit">秒</span>
      </el-form-item>
      <el-form-item label="QoS">
        <el-select v-model="uplinkForm.qos" style="width: 160px">
          <el-option :value="0" label="0 · 最多一次" />
          <el-option :value="1" label="1 · 至少一次" />
          <el-option :value="2" label="2" />
        </el-select>
      </el-form-item>
    </el-form>
    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="HTTP / OPC UA 上行需在「配置管理 › 运行参数」按分节配置"
      description="本步只写 MQTT 主通道（网关默认北向）。其余北向通道各自有独立配置节与启用开关。"
    />
  </div>
</template>

<script setup lang="ts">
/**
 * 部署向导 · 第 4 步：配置上行转发。
 *
 * 本步只写 MQTT 主通道（网关默认北向），其余北向通道在「运行参数」页按分节配置。
 *
 * Props:
 *   - uplinkForm: 上行转发表单值（v-model：enabled / broker / topic / publishInterval / qos）
 *
 * 典型应用场景：部署向导的第 4 步，只负责渲染与局部交互，不直接发请求。
 */
import type { UplinkFormModel } from './types'

/** 上行转发表单值 */
const uplinkForm = defineModel<UplinkFormModel>('uplinkForm', { required: true })
</script>

<style scoped>
.field-hint { font-size: 0.75rem; color: var(--text-secondary); line-height: 1.5; margin-top: 2px; }
.unit { margin-left: 8px; font-size: 0.75rem; color: var(--text-secondary); }
</style>
