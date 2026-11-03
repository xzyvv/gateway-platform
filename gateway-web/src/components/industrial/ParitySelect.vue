<template>
  <el-select
    :model-value="modelValue"
    :disabled="disabled"
    :placeholder="placeholder"
    :clearable="clearable"
    :style="{ width }"
    :aria-label="ariaLabel"
    @update:model-value="$emit('update:modelValue', $event)"
  >
    <el-option label="无校验（None）" value="none" />
    <el-option label="偶校验（Even）" value="even" />
    <el-option label="奇校验（Odd）" value="odd" />
  </el-select>
</template>

<script setup lang="ts">
/** 串口校验位选择器。
 *  - 3 种校验方式：无 / 偶 / 奇
 *    （网关串口层 serialport::Parity 仅支持 None/Odd/Even，不含 Mark/Space）
 *  - 标签含中文名称 + 英文简称
 *
 * Props:
 *  - modelValue:  v-model 绑定（'none' | 'even' | 'odd'）
 *  - disabled:    是否禁用
 *  - placeholder: 默认 "请选择校验位"
 *  - width:       默认 100%
 *  - clearable:   默认 false
 *  - ariaLabel:   无障碍名称，默认 "校验位"
 */
withDefaults(defineProps<{
  modelValue: string | null
  disabled?: boolean
  placeholder?: string
  width?: string
  clearable?: boolean
  ariaLabel?: string
}>(), {
  disabled: false,
  placeholder: '请选择校验位',
  width: '100%',
  clearable: false,
  ariaLabel: '校验位',
})

defineEmits<{
  'update:modelValue': [value: string | null]
}>()
</script>