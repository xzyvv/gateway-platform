<template>
  <div class="modbus-addr">
    <el-select
      v-model="region"
      :style="{ width: regionWidth }"
      :disabled="disabled"
      class="modbus-region"
      aria-label="地址区段"
      @change="onChange"
    >
      <el-option label="0xxxx (线圈)" value="0xxxx" />
      <el-option label="1xxxx (离散输入)" value="1xxxx" />
      <el-option label="3xxxx (保持寄存器)" value="3xxxx" />
      <el-option label="4xxxx (输入寄存器)" value="4xxxx" />
    </el-select>
    <el-input-number
      v-model="offset"
      :min="0"
      :max="9999"
      :step="1"
      :disabled="disabled"
      :style="{ width: offsetWidth }"
      class="modbus-offset"
      aria-label="地址偏移"
      @change="onChange"
    />
    <span class="modbus-sep">→</span>
    <el-tag class="modbus-summary" :type="tagType">{{ summaryText }}</el-tag>
  </div>
</template>

<script setup lang="ts">
/** Modbus 地址输入器。
 *
 *  - 左：4 区段下拉（0xxxx / 1xxxx / 3xxxx / 4xxxx）
 *  - 中：0-9999 偏移数字输入框
 *  - 右：实时汇总显示"功能码 + 完整 5 位地址"
 *
 *  Modbus 地址规范：
 *    - 0xxxx (1-9999)：线圈，功能码 01/05/15
 *    - 1xxxx (1-9999)：离散输入，功能码 02
 *    - 3xxxx (1-9999)：保持寄存器，功能码 03/06/16
 *    - 4xxxx (1-9999)：输入寄存器，功能码 04
 *
 *  - modelValue 为 5 位数字字符串（如 "40001"）
 *  - 支持 props 默认值自定义
 *
 * Props:
 *  - modelValue:  v-model 绑定（5 位数字字符串）
 *  - disabled:    是否禁用
 *  - regionWidth: 区段选择框宽度，默认 140px
 *  - offsetWidth: 偏移输入框宽度，默认 120px
 */
import { ref, computed, watch } from 'vue'

type Region = '0xxxx' | '1xxxx' | '3xxxx' | '4xxxx'

const REGION_MAP: Record<Region, { code: string; min: number; max: number }> = {
  '0xxxx': { code: '01/05/15', min: 1, max: 9999 },
  '1xxxx': { code: '02', min: 1, max: 9999 },
  '3xxxx': { code: '03/06/16', min: 1, max: 9999 },
  '4xxxx': { code: '04', min: 1, max: 9999 },
}

const props = withDefaults(defineProps<{
  modelValue: string
  disabled?: boolean
  regionWidth?: string
  offsetWidth?: string
}>(), {
  disabled: false,
  regionWidth: '140px',
  offsetWidth: '120px',
})

const emit = defineEmits<{
  'update:modelValue': [value: string]
}>()

const region = ref<Region>('4xxxx')
const offset = ref<number>(1)

function parse(v: string) {
  if (!v || v.length < 5) {
    region.value = '4xxxx'
    offset.value = 1
    return
  }
  const head = v[0]
  if (head === '0' || head === '1' || head === '3' || head === '4') {
    region.value = `${head}xxxx` as Region
    offset.value = parseInt(v.slice(1), 10) || 1
  }
}

parse(props.modelValue)
watch(() => props.modelValue, parse)

const tagType = computed<'success' | 'warning' | 'info' | 'danger'>(() => {
  switch (region.value) {
    case '0xxxx': return 'success'
    case '1xxxx': return 'warning'
    case '3xxxx': return 'info'
    case '4xxxx': return 'info'
    default: return 'info'
  }
})

const summaryText = computed(() => {
  const r = REGION_MAP[region.value]
  const padded = String(offset.value).padStart(4, '0')
  return `${region.value[0]}${padded}（FC ${r.code}）`
})

function onChange() {
  const padded = String(offset.value).padStart(4, '0')
  emit('update:modelValue', `${region.value[0]}${padded}`)
}
</script>

<style scoped>
.modbus-addr { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.modbus-region { flex-shrink: 0; }
.modbus-offset { flex-shrink: 0; }
.modbus-sep { color: var(--text-secondary); font-size: 0.8125rem; }
.modbus-summary { font-family: var(--font-mono); }
</style>