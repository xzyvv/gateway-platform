<template>
  <el-form v-if="mqtt" :model="mqtt" label-width="140px" class="mqtt-form">
    <!-- 基础连接：日常接入必填，平铺直出 -->
    <el-form-item label="启用 MQTT">
      <el-switch v-model="mqtt.enabled" active-text="启用" inactive-text="禁用" />
    </el-form-item>
    <el-form-item label="Broker 地址">
      <el-input v-model="mqtt.broker" placeholder="mqtt://broker.example.com:1883">
        <template #prefix><el-icon><ChatDotRound /></el-icon></template>
      </el-input>
    </el-form-item>
    <el-form-item label="Topic">
      <el-input v-model="mqtt.topic" placeholder="gateway/data" />
    </el-form-item>
    <el-form-item label="QoS 等级">
      <el-select v-model="mqtt.qos" style="width: 100%">
        <el-option :value="0" label="0 - 最多一次" />
        <el-option :value="1" label="1 - 至少一次" />
        <el-option :value="2" label="2 - 精确一次" />
      </el-select>
    </el-form-item>
    <el-form-item label="上报周期">
      <!-- max 以「秒」为单位（3600 秒 = 1 小时） -->
      <DurationSelect v-model="mqtt.publish_interval" :max="3600" :units="['s', 'min']" />
    </el-form-item>
    <el-form-item label="用户名">
      <el-input v-model="mqtt.username" placeholder="留空=匿名连接" clearable />
    </el-form-item>
    <el-form-item label="密码">
      <el-input v-model="mqtt.password" type="password" show-password placeholder="留空=无密码" clearable />
    </el-form-item>

    <!-- 高级参数：心跳/ACK/压缩/限速/Payload 模板属于调优项，多数部署用默认值即可，
         默认折叠避免与必填项抢注意力；折叠标题上直接摘要当前值，不展开也能看到配置现状。 -->
    <el-collapse v-model="advancedOpen" class="mqtt-advanced">
      <el-collapse-item name="advanced">
        <template #title>
          <div class="mqtt-advanced-title">
            <span class="mqtt-advanced-name">高级参数</span>
            <span class="mqtt-advanced-summary">{{ advancedSummary }}</span>
          </div>
        </template>

        <el-form-item label="心跳间隔（秒）">
          <el-input-number v-model="mqtt.keep_alive" :min="5" :max="300" />
        </el-form-item>
        <el-form-item label="ACK 超时（秒）">
          <el-input-number v-model="mqtt.ack_timeout_secs" :min="1" :max="120" />
        </el-form-item>
        <el-form-item label="压缩方式">
          <!-- 空串代理「不压缩」：el-option 的 value 只接受字符串/数字，直接绑 null 会触发 prop 类型警告 -->
          <el-select v-model="compressionProxy" style="width: 100%" clearable placeholder="不压缩">
            <el-option value="" label="无" />
            <el-option value="gzip" label="gzip" />
            <el-option value="zstd" label="zstd" />
          </el-select>
        </el-form-item>
        <el-form-item label="压缩阈值（字节）">
          <el-input-number v-model="mqtt.compression_threshold" :min="0" placeholder="0=始终压缩" />
        </el-form-item>
        <el-form-item label="续传限速（条/秒）">
          <el-input-number v-model="mqtt.resume_rate_limit" :min="0" placeholder="0=不限速" />
        </el-form-item>
        <el-form-item label="Payload 模板">
          <el-input v-model="mqtt.payload_template" type="textarea" :rows="3" placeholder="留空=默认 JSON 格式" />
        </el-form-item>
      </el-collapse-item>
    </el-collapse>
  </el-form>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import DurationSelect from '@/components/industrial/DurationSelect.vue'
import type { MqttConfig } from '@/api/types'

const mqtt = defineModel<MqttConfig>({ required: true })

/** 高级分组默认收起：这些参数多数部署用默认值即可 */
const advancedOpen = ref<string[]>([])

/**
 * 压缩方式的代理值：下拉里用空串表示「不压缩」，写回配置时还原为后端约定的 null。
 * 清空（clearable）同样落到 ''，统一转成 null，避免保存出空字符串。
 */
const compressionProxy = computed<string>({
  get: () => mqtt.value?.compression ?? '',
  set: (v) => {
    if (!mqtt.value) return
    mqtt.value.compression = v === '' ? null : v
  },
})

/** 折叠状态下仍能看到当前高级参数取值，不必展开就能确认配置是否生效 */
const advancedSummary = computed(() => {
  const m = mqtt.value
  if (!m) return ''
  // null 表示用后端默认值（心跳/ACK 未显式配置时如此），摘要里写「默认」而不是空值
  const fmt = (v: number | null | undefined, unit = '') =>
    v === null || v === undefined ? '默认' : `${v}${unit}`
  const parts = [
    `心跳 ${fmt(m.keep_alive, 's')}`,
    `ACK ${fmt(m.ack_timeout_secs, 's')}`,
    `压缩 ${m.compression || '不压缩'}`,
  ]
  if (m.resume_rate_limit) parts.push(`限速 ${m.resume_rate_limit} 条/秒`)
  if (m.payload_template) parts.push('自定义 Payload')
  return parts.join(' · ')
})
</script>

<style scoped>
.mqtt-advanced {
  margin-top: 4px;
}
.mqtt-advanced :deep(.el-collapse-item__header),
.mqtt-advanced :deep(.el-collapse-item__wrap) {
  padding-left: 0;
  padding-right: 0;
}
.mqtt-advanced-title {
  display: flex;
  align-items: baseline;
  gap: 10px;
  flex-wrap: wrap;
}
.mqtt-advanced-name {
  font-weight: 600;
}
.mqtt-advanced-summary {
  font-size: 0.75rem;
  color: var(--text-secondary);
}
</style>
