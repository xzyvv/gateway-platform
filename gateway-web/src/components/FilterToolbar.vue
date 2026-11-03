<template>
  <div class="filter-toolbar">
    <template v-for="f in fields" :key="f.key">
      <el-select
        v-if="f.type === 'select'"
        :model-value="modelValue[f.key]"
        size="small"
        filterable
        :clearable="f.clearable ?? true"
        :placeholder="f.placeholder"
        :aria-label="fieldLabel(f)"
        :style="{ width: (f.width ?? 150) + 'px' }"
        @update:model-value="(v: unknown) => setField(f.key, v, true)"
      >
        <el-option v-for="o in normOptions(f)" :key="o.value" :label="o.label" :value="o.value" />
      </el-select>
      <el-input
        v-else
        :model-value="(modelValue[f.key] as string | undefined) ?? ''"
        size="small"
        clearable
        :placeholder="f.placeholder"
        :aria-label="fieldLabel(f)"
        :style="{ width: (f.width ?? 160) + 'px' }"
        @update:model-value="(v: string) => setField(f.key, v, false)"
        @keyup.enter="triggerSearch"
        @clear="triggerSearch"
      />
    </template>

    <!-- 自定义控件（时间范围、操作按钮等） -->
    <slot />

    <template v-if="hasSearch">
      <el-button size="small" type="primary" @click="emit('search')">
        <el-icon><Search /></el-icon> 查询
      </el-button>
      <el-button size="small" @click="emit('reset')">重置</el-button>
    </template>
    <slot name="append" />
  </div>
</template>

<script lang="ts">
/** 筛选字段描述 */
export interface FilterFieldOption {
  label: string
  value: string | number
}

export interface FilterField {
  /** 对应 v-model 对象的键 */
  key: string
  type: 'input' | 'select'
  placeholder?: string
  /** 无障碍名称（读屏器播报用）；缺省时回落到 placeholder，再缺省用 key */
  ariaLabel?: string
  width?: number
  /** select 可选项：原始值数组或 {label,value} 数组 */
  options?: Array<FilterFieldOption | string | number>
  /** select 是否可清空（默认可） */
  clearable?: boolean
}
</script>

<script setup lang="ts">
import { computed, useAttrs } from 'vue'
import { Search } from '@element-plus/icons-vue'

const props = defineProps<{
  /** 筛选值对象，key 与 fields 对应 */
  modelValue: Record<string, unknown>
  fields: FilterField[]
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', v: Record<string, unknown>): void
  (e: 'search'): void
  (e: 'reset'): void
}>()

// 未监听 @search 时不渲染查询/重置按钮（纯本地过滤页面）
const attrs = useAttrs()
const hasSearch = computed(() => !!attrs.onSearch)

function setField(key: string, v: unknown, trigger: boolean) {
  emit('update:modelValue', { ...props.modelValue, [key]: v })
  if (trigger) triggerSearch()
}

// select 变更 / input 回车与清空时触发查询（对齐既有页面交互）
function triggerSearch() {
  if (hasSearch.value) emit('search')
}

function normOptions(f: FilterField): FilterFieldOption[] {
  return (f.options ?? []).map((o) => (typeof o === 'object' ? o : { label: String(o), value: o }))
}

/**
 * 字段的无障碍名称：ariaLabel → placeholder → key。
 * placeholder 常带提示文案（如「选择设备（可搜索）」），作名称不够干净，
 * 因此优先用 ariaLabel；调用方未给时用 placeholder 兜底，保证读屏器至少有名字。
 */
function fieldLabel(f: FilterField): string {
  return f.ariaLabel || f.placeholder || f.key
}
</script>

<style scoped>
.filter-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
</style>
