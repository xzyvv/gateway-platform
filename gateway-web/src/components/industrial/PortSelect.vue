<template>
  <el-select
    :model-value="modelValue"
    :disabled="disabled"
    :placeholder="placeholder"
    :clearable="clearable"
    filterable
    allow-create
    default-first-option
    :style="{ width }"
    :aria-label="ariaLabel"
    @update:model-value="$emit('update:modelValue', $event)"
  >
    <!-- 端口是现场部署最容易填错的一项：新手既不知道本机有哪些端口，
         也不知道该写 COM3 还是 /dev/ttyUSB0。这里按平台分组给出常见候选，
         并允许直接输入自定义值（allow-create）覆盖 COM10+ / 多路 USB 等场景。 -->
    <el-option-group label="Windows 串口">
      <el-option v-for="p in WINDOWS_PORTS" :key="p" :label="p" :value="p" />
    </el-option-group>
    <el-option-group label="Linux / 工控机">
      <el-option v-for="p in UNIX_PORTS" :key="p" :label="p" :value="p" />
    </el-option-group>
  </el-select>
</template>

<script setup lang="ts">
import { ElOption, ElOptionGroup, ElSelect } from 'element-plus'

/**
 * 串口端口选择器。
 *  - 常见候选按平台分组，避免用户凭记忆手打
 *  - allow-create：可直接输入 COM23、/dev/ttyUSB5 等未列出的值
 *  - filterable：候选较多时可搜索
 *
 * Props:
 *  - modelValue:  v-model 绑定（端口名，如 "COM3" / "/dev/ttyUSB0"）
 *  - disabled:    是否禁用
 *  - placeholder: 默认给出两种平台的写法示例
 *  - width:       默认 100%
 *  - clearable:   默认 false
 *  - ariaLabel:   无障碍名称，默认「串口端口」
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
  placeholder: '选择或输入，如 COM3 / /dev/ttyUSB0',
  width: '100%',
  clearable: false,
  ariaLabel: '串口端口',
})

defineEmits<{
  'update:modelValue': [value: string | null]
}>()

const WINDOWS_PORTS = ['COM1', 'COM2', 'COM3', 'COM4', 'COM5', 'COM6', 'COM7', 'COM8']
const UNIX_PORTS = ['/dev/ttyUSB0', '/dev/ttyUSB1', '/dev/ttyUSB2', '/dev/ttyUSB3', '/dev/ttyS0', '/dev/ttyS1', '/dev/ttyACM0']
</script>
