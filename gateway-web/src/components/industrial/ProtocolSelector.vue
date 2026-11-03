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
    <el-option
      v-for="opt in filteredOptions"
      :key="opt.value"
      :label="opt.label"
      :value="opt.value"
    />
  </el-select>
</template>

<script setup lang="ts">
/**
 * 协议选择器
 *
 * 统一封装 el-select + protocolOptions，避免每个表单各自维护 el-option v-for。
 *
 * 数据源：composables/devices/useProtocol.ts 的 protocolOptions（单一来源）
 * 新增协议只需更新 useProtocol.ts，所有调用方自动同步。
 *
 * Props:
 *   - modelValue: v-model 绑定的协议值（如 'modbus'）
 *   - filter:     可选白名单，只展示指定协议子集（如 ['modbus', 'dlt645']）
 *   - disabled:   是否禁用
 *   - placeholder: 占位文字，默认 "请选择协议"
 *   - width:      选择器宽度，默认 "100%"
 *   - clearable:   是否允许清空，默认 false
 *   - ariaLabel:   无障碍名称，默认 "协议"
 *
 * 示例：
 *   <ProtocolSelector v-model="form.protocol" />
 *   <ProtocolSelector v-model="discover.protocol" :filter="['modbus', 'cjt188']" width="140px" />
 */
import { computed } from 'vue'
import { protocolOptions } from '@/composables/devices/useProtocol'

const props = withDefaults(defineProps<{
  modelValue: string
  filter?: string[]
  disabled?: boolean
  placeholder?: string
  width?: string
  clearable?: boolean
  ariaLabel?: string
}>(), {
  disabled: false,
  placeholder: '请选择协议',
  width: '100%',
  clearable: false,
  ariaLabel: '协议',
})

defineEmits<{
  'update:modelValue': [value: string]
}>()

const filteredOptions = computed(() => {
  if (!props.filter || props.filter.length === 0) {
    return protocolOptions
  }
  return protocolOptions.filter(opt => props.filter!.includes(opt.value))
})
</script>