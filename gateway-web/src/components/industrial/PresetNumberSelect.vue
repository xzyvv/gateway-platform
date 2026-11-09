<template>
  <el-select
    :model-value="displayValue"
    :style="{ width }"
    :disabled="disabled"
    filterable
    allow-create
    default-first-option
    :placeholder="placeholder"
    :aria-label="ariaLabel"
    @update:model-value="onUpdate"
  >
    <el-option v-for="n in mergedOptions" :key="n" :label="labelOf(n)" :value="String(n)" />
  </el-select>
</template>

<script setup lang="ts">
/** 数值「档位」选择器：常用档位下拉 + 允许输入自定义值。
 *
 * 适用场景：后端只约束区间、现场通常从几个常见档位里挑的整数参数
 * （队列行数、限流阈值、缓冲深度、心跳间隔等）。相比裸数字框，
 * 下拉能直接展示「默认值/推荐档位」，也避免手滑多敲一个 0。
 *
 * 实现约定：
 *  - 选项与模型值统一按字符串传给 el-select（allow-create 只会产出字符串），
 *    写回前再转成数字并按 min/max 收敛；
 *  - 当前值不在预设内时动态补一条选项，保证任意合法配置都能正确回显；
 *  - `null` 表示「未显式配置」（走后端默认值），此时显示 placeholder。
 *
 * Props:
 *  - modelValue:  v-model 绑定值（整数；null 表示未配置）
 *  - options:     预设档位
 *  - min / max:   取值区间，输入越界会被收敛到边界
 *  - unit:        单位后缀，仅影响展示（无 format 时生效）
 *  - format:      自定义展示文案，如「100 万行」，优先于 unit
 *  - width:       宽度，默认 100%
 *  - placeholder: 未配置时的占位文案
 *  - ariaLabel:   无障碍名称
 */
import { computed } from 'vue'

const props = withDefaults(defineProps<{
  modelValue?: number | null
  options: number[]
  min?: number
  max?: number
  unit?: string
  format?: (n: number) => string
  width?: string
  placeholder?: string
  ariaLabel?: string
  disabled?: boolean
}>(), {
  modelValue: null,
  min: undefined,
  max: undefined,
  unit: '',
  format: undefined,
  width: '100%',
  placeholder: '选择或输入自定义值',
  ariaLabel: '数值档位',
  disabled: false,
})

const emit = defineEmits<{
  'update:modelValue': [value: number]
}>()

/** 当前值也作为选项补进去：历史配置可能落在预设之外，否则下拉会显示空白 */
const mergedOptions = computed<number[]>(() => {
  const list = [...props.options]
  const current = props.modelValue
  if (current != null && !list.includes(current)) list.push(current)
  return list
})

/** null/undefined 交给 el-select 显示 placeholder */
const displayValue = computed(() =>
  props.modelValue == null ? undefined : String(props.modelValue),
)

function labelOf(n: number): string {
  if (props.format) return props.format(n)
  return props.unit ? `${n} ${props.unit}` : String(n)
}

/** allow-create 只会给出字符串，统一转数字并收敛到 [min, max] */
function onUpdate(v: string | number | null | undefined) {
  if (v == null) return
  const n = Number(String(v).trim())
  if (!Number.isFinite(n)) return
  let next = Math.round(n)
  if (props.min !== undefined) next = Math.max(next, props.min)
  if (props.max !== undefined) next = Math.min(next, props.max)
  if (next === props.modelValue) return
  emit('update:modelValue', next)
}
</script>
