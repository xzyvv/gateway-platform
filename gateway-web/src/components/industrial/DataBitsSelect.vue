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
    <el-option label="5 位（部分老设备）" :value="5" />
    <el-option label="6 位（少见）" :value="6" />
    <el-option label="7 位（标准 ASCII）" :value="7" />
    <el-option label="8 位（最常用）" :value="8" />
  </el-select>
</template>

<script setup lang="ts">
/** 串口数据位选择器。
 *  - 4 种数据位长度：5 / 6 / 7 / 8
 *  - 工业常用：8（现代设备）/ 7（ASCII 老设备）
 *
 * Props:
 *  - modelValue:  v-model 绑定（5 | 6 | 7 | 8）
 *  - disabled:    是否禁用
 *  - placeholder: 默认 "请选择数据位"
 *  - width:       默认 100%
 *  - clearable:   默认 false
 *  - ariaLabel:   无障碍名称，默认 "数据位"
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
  placeholder: '请选择数据位',
  width: '100%',
  clearable: false,
  ariaLabel: '数据位',
})

defineEmits<{
  'update:modelValue': [value: number | null]
}>()
</script>