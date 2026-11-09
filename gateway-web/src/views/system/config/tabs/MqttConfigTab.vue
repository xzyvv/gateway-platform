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
    <el-form-item label="告警 Topic">
      <!-- 告警 Topic 由数据 Topic 派生，只读展示，避免对接方猜测订阅地址 -->
      <div class="derived-topics">
        <el-input :model-value="alarmTopic" readonly placeholder="点位报警/恢复" />
        <el-input :model-value="systemAlarmTopic" readonly placeholder="系统（磁盘）告警" />
        <div class="derived-hint">由数据 Topic 派生，不可单独配置</div>
      </div>
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
          <PresetNumberSelect
            v-model="mqtt.keep_alive"
            :options="KEEP_ALIVE_PRESETS"
            :min="5"
            :max="300"
            unit="秒"
            placeholder="默认 30"
            aria-label="心跳间隔"
          />
        </el-form-item>
        <el-form-item label="ACK 超时（秒）">
          <PresetNumberSelect
            v-model="mqtt.ack_timeout_secs"
            :options="ACK_TIMEOUT_PRESETS"
            :min="1"
            :max="120"
            unit="秒"
            placeholder="默认 10"
            aria-label="ACK 超时"
          />
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
        <el-form-item label="CA 证书路径">
          <el-input
            :model-value="mqtt.tls?.ca_cert_path ?? ''"
            placeholder="留空 = 使用系统根证书"
            clearable
            @update:model-value="onCaCertInput"
          />
          <div class="tls-hint">
            仅 Broker 使用 TLS（mqtts/wss）时生效；填 PEM/DER 证书的绝对路径，用于自签或企业私有 CA。
          </div>
        </el-form-item>
        <el-form-item label="重连退避">
          <!-- 逐档秒数是调优项，前端只给三个预设；现场特殊需求仍可直接改 config.json -->
          <div class="backoff-field">
            <el-select v-model="backoffPreset" style="width: 100%">
              <el-option
                v-for="opt in BACKOFF_OPTIONS"
                :key="opt.value"
                :value="opt.value"
                :label="opt.label"
              />
            </el-select>
            <div class="backoff-hint">
              当前：{{ backoff.seconds.join(' / ') }} 秒，抖动 {{ Math.round(backoff.jitter_ratio * 100) }}%
            </div>
          </div>
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
import PresetNumberSelect from '@/components/industrial/PresetNumberSelect.vue'
import type { MqttConfig } from '@/api/types'
import {
  BACKOFF_LABELS,
  BACKOFF_OPTIONS,
  BACKOFF_PRESETS,
  applyPreset,
  presetOf,
  type Backoff,
  type BackoffPreset,
} from '@/utils/backoffPresets'

const mqtt = defineModel<MqttConfig>({ required: true })

/** 高级分组默认收起：这些参数多数部署用默认值即可 */
const advancedOpen = ref<string[]>([])

/** 心跳常用档位（后端默认 30 秒，校验区间 5~300） */
const KEEP_ALIVE_PRESETS = [15, 30, 60, 120, 180, 300]
/** ACK 超时常用档位（后端默认 10 秒，校验区间 1~120） */
const ACK_TIMEOUT_PRESETS = [5, 10, 15, 30, 60, 120]

/**
 * 点位报警/恢复主题（后端 `format!("{}/alarm", topic)`）。
 *
 * 刻意不做斜杠归一化：后端是直接字符串拼接，展示必须与真实下发主题一致，
 * 否则用户按这里显示的地址去订阅会收不到消息。
 */
const alarmTopic = computed(() => (mqtt.value?.topic ? `${mqtt.value.topic}/alarm` : ''))

/** 系统告警主题（后端 `{topic}/system/alarm`，当前用于磁盘告警） */
const systemAlarmTopic = computed(() =>
  mqtt.value?.topic ? `${mqtt.value.topic}/system/alarm` : '',
)

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

/**
 * 重连退避：预设与总线页共用（见 @/utils/backoffPresets），避免两处档位漂移。
 */
const backoff = computed<Backoff>(() => mqtt.value?.reconnect_backoff ?? BACKOFF_PRESETS.standard)

const backoffPreset = computed<BackoffPreset>({
  get: () => presetOf(backoff.value),
  set: (preset) => {
    // 「自定义」只是展示态：选中时不改写配置，避免覆盖现场手调的档位
    const next = applyPreset(preset)
    if (!next || !mqtt.value) return
    mqtt.value.reconnect_backoff = next
  },
})

/**
 * 自定义 CA 证书路径。
 *
 * - 清空时把 `tls` 整体置 null，恢复「使用系统根证书」的旧语义；
 * - 只暴露 ca_cert_path：insecure_skip_verify 会被后端校验直接拒绝，不提供开关；
 * - 后端用 rumqttc 建链，改完保存即重建客户端（连不上会保留旧连接）。
 */
function onCaCertInput(v: string) {
  if (!mqtt.value) return
  const path = v.trim()
  if (!path) {
    mqtt.value.tls = null
    return
  }
  mqtt.value.tls = {
    ca_cert_path: path,
    insecure_skip_verify: mqtt.value.tls?.insecure_skip_verify ?? false,
  }
}

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
    `退避 ${BACKOFF_LABELS[backoffPreset.value]}`,
  ]
  if (m.resume_rate_limit) parts.push(`限速 ${m.resume_rate_limit} 条/秒`)
  if (m.tls?.ca_cert_path) parts.push('自签 CA')
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
.derived-topics {
  display: flex;
  flex-direction: column;
  gap: 6px;
  width: 100%;
}
.derived-topics :deep(.el-input__inner) {
  color: var(--text-secondary);
}
.derived-hint {
  font-size: 0.75rem;
  color: var(--text-secondary);
}
.backoff-field {
  width: 100%;
}
.backoff-hint {
  margin-top: 4px;
  font-size: 0.75rem;
  color: var(--text-secondary);
}
.tls-hint {
  margin-top: 4px;
  font-size: 0.75rem;
  line-height: 1.5;
  color: var(--text-secondary);
}
</style>
