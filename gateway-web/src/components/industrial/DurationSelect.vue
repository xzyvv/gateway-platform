<template>
  <div class="duration-select">
    <el-input-number
      v-model="valueNum"
      :min="displayMin"
      :max="displayMax"
      :step="step"
      :disabled="disabled"
      :style="{ width: numWidth }"
      class="duration-num"
      aria-label="时长数值"
    />
    <el-select
      v-model="unitModel"
      :disabled="disabled"
      :style="{ width: unitWidth }"
      class="duration-unit"
      aria-label="时长单位"
    >
      <el-option
        v-for="opt in availableUnits"
        :key="opt.value"
        :label="opt.label"
        :value="opt.value"
      />
    </el-select>
  </div>
</template>

<script setup lang="ts">
/** 时长选择器（双段：数字 + 单位）。
 *
 * 统一承载「周期 / 间隔 / 超时」类配置：数字 + 单位自由组合，支持 ms/s/min/h/d，
 * 自动转秒写入 modelValue。毫秒档用于允许亚秒级的字段（如 Webhook 上报间隔）。
 *
 * 典型场景：
 *  - MQTT 上报周期（秒）
 *  - Webhook 上报间隔（毫秒）
 *  - NTP 同步间隔（秒）
 *  - Telemetry 指标间隔（秒）
 *  - OTA 升级超时（分）
 *  - 证书有效期（天）
 *
 * ## 单位契约（重要）
 *
 * `modelValue` / `min` / `max` 三者**始终以秒为单位**，与展示单位无关。
 * `unit` 只决定「这个秒数怎么显示给用户」，组件内部负责换算。
 *
 * 为什么必须这么约定：el-input-number 在挂载时会把越界的 modelValue 钳制后
 * 回写（见 element-plus 的 verifyValue(value, true)），若把「秒」直接塞进
 * 一个按「分钟」标定的 max 里，加载配置的瞬间就会把 3600 静默改成 1440，
 * 进而触发 dirtyGuard 误报，用户点保存时更会真的写坏网关配置。
 * 因此 min/max 在这里先换算成展示单位，再交给 el-input-number，
 * 保证「数值合法」与「显示单位」永远一致，加载即不再发生回写。
 *
 * Props:
 *  - modelValue:  v-model 绑定值（number，单位恒为秒）
 *  - unit:        展示单位，必须在 units 内；缺省或不在 units 内时取 units[0]
 *  - units:       允许的单位列表，默认 s/min/h/d
 *  - min:         最小值（秒），默认 1
 *  - max:         最大值（秒），默认 999999
 *  - step:        数字步长（展示单位的步长），默认 1
 *  - disabled:    是否禁用
 *  - numWidth:    数字输入框宽度，默认 110px
 *  - unitWidth:   单位选择框宽度，默认 100px
 */
import { computed, ref, watch } from 'vue'

type DurationUnit = 'ms' | 's' | 'min' | 'h' | 'd'

const props = withDefaults(defineProps<{
  modelValue: number
  unit?: DurationUnit
  units?: DurationUnit[]
  /** 最小值，单位：秒 */
  min?: number
  /** 最大值，单位：秒 */
  max?: number
  step?: number
  disabled?: boolean
  numWidth?: string
  unitWidth?: string
}>(), {
  unit: 's',
  units: () => ['s', 'min', 'h', 'd'] as DurationUnit[],
  min: 1,
  max: 999999,
  step: 1,
  disabled: false,
  numWidth: '110px',
  unitWidth: '100px',
})

const emit = defineEmits<{
  'update:modelValue': [value: number]
  'update:unit': [value: DurationUnit]
}>()

const UNIT_LABEL: Record<DurationUnit, string> = {
  ms: '毫秒',
  s: '秒',
  min: '分钟',
  h: '小时',
  d: '天',
}

const UNIT_FACTOR: Record<DurationUnit, number> = {
  ms: 0.001,
  s: 1,
  min: 60,
  h: 3600,
  d: 86400,
}

/** 校验并归一化单位：不在可选集合内的一律回退到首个选项 */
function resolveUnit(u: DurationUnit, list: DurationUnit[]): DurationUnit {
  return list.includes(u) ? u : (list[0] ?? 's')
}

/** 当前展示单位：本地可改（下拉切换），同时跟随 props.unit 变化 */
const activeUnit = ref<DurationUnit>(resolveUnit(props.unit, props.units))
// 用字符串键做依赖：父组件常用内联数组（:units="['s','min']"），每次渲染都是新引用，
// 直接 watch 数组会导致用户切换单位后被下一帧重置回首个单位
watch(
  () => `${props.unit}|${props.units.join(',')}`,
  () => { activeUnit.value = resolveUnit(props.unit, props.units) },
)

const factor = computed(() => UNIT_FACTOR[activeUnit.value])

/** 展示的数字（= 内部秒值 / 当前单位因子） */
const valueNum = computed<number>({
  get() {
    return Math.round(props.modelValue / factor.value)
  },
  set(v: number) {
    // 毫秒档下秒值是小数：按毫秒精度（3 位）收敛，避免被取整成整秒
    emit('update:modelValue', Math.round(v * factor.value * 1000) / 1000)
  },
})

/** 展示单位下的最小值（秒 → 当前单位，向上取整，保证不小于真实下限） */
const displayMin = computed(() => Math.ceil(props.min / factor.value))

/**
 * 展示单位下的最大值（秒 → 当前单位，向下取整，保证不超过真实上限）。
 * 下限兜底为 displayMin：单位变大后 max 可能取整为 0，
 * 此时若 max < min，el-input-number 会直接抛错。
 */
const displayMax = computed(() => Math.max(displayMin.value, Math.floor(props.max / factor.value)))

/** 单位下拉的双向绑定代理：写回 activeUnit 并抛 update:unit（避免与同名 prop `unit` 冲突） */
const unitModel = computed<DurationUnit>({
  get() { return activeUnit.value },
  set(v: DurationUnit) {
    if (v === activeUnit.value) return
    activeUnit.value = v
    emit('update:unit', v)
  },
})

const availableUnits = computed(() =>
  props.units.map((u) => ({ value: u, label: UNIT_LABEL[u] })),
)
</script>

<style scoped>
.duration-select {
  display: flex;
  gap: 8px;
  align-items: center;
}
.duration-num { flex-shrink: 0; }
.duration-unit { flex-shrink: 0; }
</style>
