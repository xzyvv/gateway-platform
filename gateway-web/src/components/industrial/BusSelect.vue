<template>
  <el-select
    :model-value="modelValue"
    :style="{ width: widthStyle }"
    :size="size"
    :loading="loading"
    :disabled="disabled"
    :filterable="filterable"
    :allow-create="allowCreate"
    :default-first-option="defaultFirstOption"
    :placeholder="placeholder"
    :aria-label="ariaLabel"
    :no-data-text="noDataText"
    @update:model-value="onUpdate"
    @change="onChange"
  >
    <template v-if="$slots.prefix" #prefix>
      <slot name="prefix" />
    </template>
    <el-option v-for="o in options" :key="o.value" :label="o.label" :value="o.value">
      <span>{{ o.label }}</span>
      <span v-if="o.meta" class="bus-opt-meta">{{ o.meta }}</span>
    </el-option>
  </el-select>
</template>

<script setup lang="ts">
/**
 * 总线下拉（统一件）
 *
 * 收敛前，同一条「选总线」的 el-select 在调试台/扫描页/抓包页/设备表单/批量编辑里
 * 各写了一遍（9 处），差异全在细节上，而细节恰好是现场踩坑的地方：
 * - `no-data-text`：有的写了「暂无总线，请先在配置管理中添加」，有的没写，
 *   后者在未配置总线时只弹一个空面板，用户不知道是「没配」还是「加载失败」；
 * - `aria-label`：有的叫「总线」有的叫「调试目标总线」，读屏用户听到的名字与
 *   页面上的可见标签对不上；
 * - 宽度 200 / 220 混用，同一页两个下拉框宽度不一致；
 * - 选项文案：有的把协议拼进 label（`/dev/ttyUSB0 (modbus-rtu)`），
 *   有的把协议放在右侧灰色 meta 里。两种都对，但同一处界面要一致。
 *
 * 本组件只负责「渲染 + 透传事件」，选项由调用方给（各页数据源不同：
 * 调试页来自 useDebugTargets 的共享快照，表单来自 /api/config 的 buses）。
 *
 * Props:
 * - modelValue: 当前总线标识（通常是 port_name）
 * - options:    选项列表；`meta` 会渲染成右侧灰色小字（如协议名）
 * - loading / disabled / filterable: 透传 el-select
 * - width:      宽度，数字按 px 处理；默认 220（收敛前的两种取值之一）
 * - placeholder / ariaLabel / noDataText / size: 文案与尺寸，默认值与多数调用点一致
 *
 * Slots:
 * - prefix: 透传给 el-select 的前置插槽（点位批量编辑用它放「是否修改」勾选框）
 *
 * Emits:
 * - update:modelValue / change
 */
import { computed } from 'vue'

/** 总线选项：`meta` 为右侧灰色说明（协议名等），不参与搜索与提交值 */
export interface BusOption {
  label: string
  value: string
  meta?: string
}

const props = withDefaults(defineProps<{
  modelValue: string
  options: BusOption[]
  loading?: boolean
  disabled?: boolean
  /** 可搜索：总线数量多（或名字相近的串口）时必开，否则滚半天找不到 */
  filterable?: boolean
  /** 允许输入列表外的值（如透传目标总线：可以先填端口名，稍后再回来配总线） */
  allowCreate?: boolean
  /** 配合 allowCreate：回车即选中当前输入项，少按一次方向键 */
  defaultFirstOption?: boolean
  placeholder?: string
  ariaLabel?: string
  /** 空列表提示：必须说清「没配」而不是留一个空面板让人以为是加载失败 */
  noDataText?: string
  width?: number | string
  size?: 'small' | 'default' | 'large'
}>(), {
  loading: false,
  disabled: false,
  filterable: false,
  allowCreate: false,
  defaultFirstOption: false,
  placeholder: '选择总线',
  ariaLabel: '总线',
  noDataText: '暂无总线，请先在配置管理中添加',
  width: 220,
})

const emit = defineEmits<{
  (e: 'update:modelValue', v: string): void
  (e: 'change', v: string): void
}>()

// 具名处理器而不是模板内联箭头：内联箭头每次渲染都是新函数，
// el-select 内部对 listener 身份变化比较敏感（会触发额外的重算）。
function onUpdate(v: string): void {
  emit('update:modelValue', v)
}
function onChange(v: string): void {
  emit('change', v)
}

/** 数字按 px 处理（`width: 220` → `220px`），也允许调用方直接给 `'100%'` */
const widthStyle = computed(() =>
  typeof props.width === 'number' ? `${props.width}px` : props.width,
)
</script>

<style scoped>
/* 协议名等附加信息：靠右灰字，不抢主文案 */
.bus-opt-meta {
  float: right;
  margin-left: 16px;
  font-size: 0.75rem;
  color: var(--text-placeholder);
}
</style>
