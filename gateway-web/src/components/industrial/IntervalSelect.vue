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
    <el-option-group label="毫秒">
      <el-option v-for="opt in MS_PRESETS" :key="opt.value" :label="opt.label" :value="opt.value" />
    </el-option-group>
    <el-option-group label="秒">
      <el-option v-for="opt in S_PRESETS" :key="opt.value" :label="opt.label" :value="opt.value" />
    </el-option-group>
    <el-option-group label="分钟">
      <el-option v-for="opt in MIN_PRESETS" :key="opt.value" :label="opt.label" :value="opt.value" />
    </el-option-group>
  </el-select>
</template>

<script setup lang="ts">
/** 采集周期选择器。
 *  - 默认提供 12 个常用值：100/200/500ms；1/2/5/10/30s；1/5/15/30min
 *  - 统一单位为毫秒（后端通常用 ms，避免前端各自转换）
 *  - allowCreate=true 时可输入自定义值
 *
 * Props:
 *   - modelValue:  v-model 绑定值（number，单位 ms）
 *   - unit:        显示单位 ('ms' | 's' | 'min')，影响预设的展示（不影响值）
 *   - disabled:    是否禁用
 *   - placeholder: 默认 "请选择采集周期"
 *   - width:       默认 100%
 *   - clearable:   默认 false
 *   - filterable:  默认 true
 *   - allowCreate: 默认 true（自定义值常见）
 *   - ariaLabel:   无障碍名称，默认 "采集周期"
 */
withDefaults(defineProps<{
  modelValue: number | string | null
  unit?: 'ms' | 's' | 'min'
  disabled?: boolean
  placeholder?: string
  width?: string
  clearable?: boolean
  filterable?: boolean
  allowCreate?: boolean
  ariaLabel?: string
}>(), {
  unit: 'ms',
  disabled: false,
  placeholder: '请选择采集周期',
  width: '100%',
  clearable: false,
  filterable: true,
  allowCreate: true,
  ariaLabel: '采集周期',
})

defineEmits<{
  'update:modelValue': [value: number | string | null]
}>()

const MS_PRESETS = [
  { value: 100, label: '100 ms' },
  { value: 200, label: '200 ms' },
  { value: 500, label: '500 ms' },
]
const S_PRESETS = [
  { value: 1000, label: '1 秒' },
  { value: 2000, label: '2 秒' },
  { value: 5000, label: '5 秒（常用）' },
  { value: 10000, label: '10 秒' },
  { value: 30000, label: '30 秒' },
]
const MIN_PRESETS = [
  { value: 60000, label: '1 分钟' },
  { value: 300000, label: '5 分钟' },
  { value: 900000, label: '15 分钟' },
  { value: 1800000, label: '30 分钟' },
]
</script>