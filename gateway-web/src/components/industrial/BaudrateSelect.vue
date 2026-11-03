<template>
  <el-select
    :model-value="modelValue"
    :disabled="disabled"
    :placeholder="placeholder"
    :clearable="clearable"
    :filterable="filterable"
    :allow-create="allowCreate"
    :style="{ width }"
    :aria-label="ariaLabel"
    @update:model-value="$emit('update:modelValue', $event)"
  >
    <el-option
      v-for="opt in STANDARD_BAUDRATES"
      :key="opt.value"
      :label="opt.label"
      :value="opt.value"
    />
  </el-select>
</template>

<script setup lang="ts">
/** 波特率选择器。
 *  - 默认提供 9 个工业常用值：1200/2400/4800/9600/19200/38400/57600/115200/230400
 *  - allowCreate=true 时可输入自定义值（如 460800）
 *  - 数字格式：不带逗号、不带空格，统一 number 字符串
 *  - 用 el-select filterable 让用户能搜索
 *
 * Props:
 *   - modelValue:  v-model 绑定值（number 或 string）
 *   - disabled:    是否禁用
 *   - placeholder: 默认 "请选择波特率"
 *   - width:       默认 100%
 *   - clearable:   默认 false
 *   - filterable:  默认 true（支持输入搜索）
 *   - allowCreate: 默认 false（允许自定义值）
 *   - ariaLabel:   无障碍名称，默认 "波特率"（读屏器播报用；调用方可覆盖为更具体的名字）
 */
withDefaults(defineProps<{
  modelValue: number | string | null
  disabled?: boolean
  placeholder?: string
  width?: string
  clearable?: boolean
  filterable?: boolean
  allowCreate?: boolean
  ariaLabel?: string
}>(), {
  disabled: false,
  placeholder: '请选择波特率',
  width: '100%',
  clearable: false,
  filterable: true,
  allowCreate: false,
  ariaLabel: '波特率',
})

defineEmits<{
  'update:modelValue': [value: number | string | null]
}>()

const STANDARD_BAUDRATES: { value: number; label: string }[] = [
  { value: 1200, label: '1200 bps' },
  { value: 2400, label: '2400 bps' },
  { value: 4800, label: '4800 bps' },
  { value: 9600, label: '9600 bps（最常用）' },
  { value: 19200, label: '19200 bps' },
  { value: 38400, label: '38400 bps' },
  { value: 57600, label: '57600 bps' },
  { value: 115200, label: '115200 bps' },
  { value: 230400, label: '230400 bps' },
]
</script>