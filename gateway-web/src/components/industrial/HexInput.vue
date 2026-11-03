<template>
  <el-input
    :model-value="displayValue"
    :placeholder="placeholder"
    :disabled="disabled"
    :clearable="clearable"
    :status="status"
    :aria-label="ariaLabel"
    @update:model-value="onInput"
    @blur="onBlur"
  >
    <template #append>
      <el-text size="small" type="info">{{ byteCount }} 字节</el-text>
    </template>
  </el-input>
</template>

<script setup lang="ts">
/** 十六进制字符串输入器。
 *
 *  - 接受三种格式自动归一化为"01 02 03 AB"：
 *    1. "01 02 03 AB"（空格分隔）
 *    2. "010203AB"（连续无空格）
 *    3. "0x01 0x02 0x03 0xAB"（带 0x 前缀）
 *  - 输入非法字符时红色警告（status="error"）；因非法内容不写回 modelValue，
 *    同时通过 update:invalid 通知消费方（否则消费方会拿上一帧合法值静默下发）
 *  - modelValue 为大写空格分隔字符串
 *  - 字节数 0-256 校验（超过报错）
 *
 * Props:
 *  - modelValue:  v-model 绑定（空格分隔十六进制字符串）
 *  - disabled:    是否禁用
 *  - placeholder: 默认 "十六进制（如 01 02 03 AB）"
 *  - width:       默认 100%
 *  - clearable:   默认 true
 *  - maxBytes:    最大字节数限制，默认 256
 *  - ariaLabel:   无障碍名称，默认 "十六进制数据"
 */
import { ref, computed, watch } from 'vue'

const props = withDefaults(defineProps<{
  modelValue: string
  disabled?: boolean
  placeholder?: string
  width?: string
  clearable?: boolean
  maxBytes?: number
  ariaLabel?: string
}>(), {
  disabled: false,
  placeholder: '十六进制（如 01 02 03 AB）',
  width: '100%',
  clearable: true,
  maxBytes: 256,
  ariaLabel: '十六进制数据',
})

const emit = defineEmits<{
  'update:modelValue': [value: string]
  /** 当前输入是否非法（非法时不写回 modelValue，消费方须据此阻断下发） */
  'update:invalid': [invalid: boolean]
}>()

/** 内部输入缓冲（允许暂存非法字符） */
const raw = ref(props.modelValue)

watch(() => props.modelValue, (v) => {
  if (v !== raw.value) raw.value = v
})

/** 显示值 */
const displayValue = computed(() => raw.value)

/** 字节数 */
const byteCount = computed(() => {
  if (!raw.value) return 0
  const cleaned = raw.value.replace(/\s+/g, '').replace(/0x/gi, '')
  return Math.floor(cleaned.length / 2)
})

/** 校验：是否是合法 hex + 字节数未超限 */
const valid = computed(() => {
  if (!raw.value) return true
  const cleaned = raw.value.replace(/\s+/g, '').replace(/0x/gi, '')
  if (cleaned.length === 0) return true
  if (!/^[0-9A-Fa-f]*$/.test(cleaned)) return false
  if (cleaned.length % 2 !== 0) return false
  if (Math.floor(cleaned.length / 2) > props.maxBytes) return false
  return true
})

const status = computed<'success' | 'error' | undefined>(() => {
  if (raw.value && !valid.value) return 'error'
  return undefined
})

function normalize(v: string): string {
  // 去除所有 0x 前缀和空白
  const cleaned = v.replace(/\s+/g, '').replace(/0x/gi, '').toUpperCase()
  // 每两个字符插入空格
  return cleaned.match(/.{1,2}/g)?.join(' ') ?? ''
}

function onInput(v: string) {
  raw.value = v
  emit('update:invalid', !valid.value)
  if (valid.value) {
    emit('update:modelValue', normalize(v))
  }
}

function onBlur() {
  if (valid.value) {
    raw.value = normalize(raw.value)
  }
}
</script>