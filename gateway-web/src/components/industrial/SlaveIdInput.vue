<template>
  <div class="slave-id-input">
    <el-select
      v-model="selectedPreset"
      :style="{ width: presetWidth }"
      :disabled="disabled"
      class="slave-id-preset"
      filterable
      :filter-method="onFilter"
      aria-label="从站地址预设"
      @change="onPresetChange"
    >
      <el-option v-for="opt in filteredOptions" :key="opt.value" :label="opt.label" :value="opt.value" />
    </el-select>
    <el-input-number
      v-model="customValue"
      :min="0"
      :max="247"
      :step="1"
      :disabled="disabled"
      :style="{ width: customWidth }"
      class="slave-id-custom"
      aria-label="从站地址"
      @change="onCustomChange"
    />
  </div>
</template>

<script setup lang="ts">
/** Modbus 从站地址选择器。
 *  - 左：常用预设下拉（1/2/10/100/247 等，含"广播地址 0"和"自定义"）
 *  - 右：0-247 数字输入框（覆盖预设）
 *  - 两者联动：选预设填入数字框；输入数字框同步预设（无匹配则"自定义"）
 *  - filterable 支持搜索地址（按 1-247 数值匹配）
 *
 * Modbus 地址规范：
 *  - 0：广播地址（仅写）
 *  - 1-247：从站地址
 *  - 248-255：保留
 *
 * Props:
 *  - modelValue:  v-model 绑定值（number 0-247）
 *  - disabled:    是否禁用
 *  - presetWidth: 预设选择框宽度，默认 160px
 *  - customWidth: 数字输入框宽度，默认 140px
 */
import { ref, computed, watch } from 'vue'

const props = withDefaults(defineProps<{
  modelValue: number
  disabled?: boolean
  presetWidth?: string
  customWidth?: string
}>(), {
  disabled: false,
  presetWidth: '160px',
  customWidth: '140px',
})

const emit = defineEmits<{
  'update:modelValue': [value: number]
}>()

const PRESETS = [0, 1, 2, 10, 32, 100, 247]
const PRESET_LABEL = (v: number) => {
  if (v === 0) return '0（广播）'
  if (v === 247) return '247（最大）'
  return String(v)
}

const selectedPreset = ref<number | 'custom'>(
  PRESETS.includes(props.modelValue) ? props.modelValue : 'custom',
)
const customValue = ref<number>(props.modelValue)
const filterText = ref('')

const filteredOptions = computed(() => {
  const t = filterText.value
  const base = [
    ...PRESETS.map((v) => ({ value: v, label: PRESET_LABEL(v) })),
    { value: 'custom', label: '自定义' },
  ]
  if (!t) return base
  return base.filter((o) => String(o.label).includes(t) || String(o.value).includes(t))
})

function onFilter(v: string) {
  filterText.value = v
}

function onPresetChange(v: number | 'custom') {
  selectedPreset.value = v
  if (v === 'custom') return
  customValue.value = v
  emit('update:modelValue', v)
}

function onCustomChange(v: number | undefined) {
  if (v == null) return
  customValue.value = v
  selectedPreset.value = PRESETS.includes(v) ? v : 'custom'
  emit('update:modelValue', v)
}

watch(() => props.modelValue, (v) => {
  selectedPreset.value = PRESETS.includes(v) ? v : 'custom'
  customValue.value = v
})
</script>

<style scoped>
.slave-id-input { display: flex; gap: 8px; align-items: center; }
.slave-id-preset { flex-shrink: 0; }
.slave-id-custom { flex-shrink: 0; }
</style>