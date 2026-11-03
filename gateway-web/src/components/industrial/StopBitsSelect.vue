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
    <el-option label="1 位（标准）" :value="1" />
    <el-option label="1.5 位" :value="1.5" />
    <el-option label="2 位" :value="2" />
  </el-select>
</template>

<script setup lang="ts">
/** 串口停止位选择器。
 *  - 3 种停止位长度：1 / 1.5 / 2
 *  - 工业常用：1（标准）/ 2（兼容老设备）/ 1.5（少见，5 位数据位时用）
 *
 * Props:
 *  - modelValue:  v-model 绑定（1 | 1.5 | 2）
 *  - disabled:    是否禁用
 *  - placeholder: 默认 "请选择停止位"
 *  - width:       默认 100%
 *  - clearable:   默认 false
 *  - ariaLabel:   无障碍名称，默认 "停止位"
 */
withDefaults(defineProps<{
  modelValue: number | null
  disabled?: boolean
  placeholder?: string
  width?: string
  clearable?: boolean
  ariaLabel?: string
}>(), {
  disabled: false,
  placeholder: '请选择停止位',
  width: '100%',
  clearable: false,
  ariaLabel: '停止位',
})

defineEmits<{
  'update:modelValue': [value: number | null]
}>()
</script>