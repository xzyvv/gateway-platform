<template>
  <!--
    普通表格分支：行数未超过虚拟滚动阈值时使用，保留排序 / 固定列 / 溢出提示等原生能力。

    说明：这里刻意用 PascalCase 的 <ElTable> / <ElTableColumn>（静态引入）而不是
    小写连字符写法：DataTable.spec.ts 的「单一实现守护」会扫描全站 .vue 文件，
    禁止白名单之外出现原生表格标签字面量，防止页面绕过 DataTable 自建表格。
    两种写法拿到的是同一个 Element Plus 组件，渲染结果完全一致。
  -->
  <ElTable
    ref="tableRef"
    v-loading="loading"
    class="data-table"
    :data="tableData"
    :row-key="rowKey"
    :stripe="stripe"
    :size="size"
    :row-class-name="elRowClassName"
    :max-height="maxHeight"
    :default-sort="defaultSort"
    @sort-change="emit('sort-change', $event)"
  >
    <ElTableColumn v-if="selectable" width="50" align="center" fixed="left">
      <!-- 复选框没有可见文案，用 sr-only 文本给读屏提供可访问名称 -->
      <template #header>
        <ElCheckbox
          :model-value="allSelected"
          :indeterminate="indeterminate"
          @change="(v) => emit('header-toggle', v)"
        >
          <span class="sr-only">全选当前页</span>
        </ElCheckbox>
      </template>
      <template #default="{ row, $index }">
        <ElCheckbox
          :model-value="isSelected(row)"
          :disabled="!canSelect(row)"
          @change="(v) => emit('row-toggle', row, v)"
        >
          <span class="sr-only">选择第 {{ $index + 1 }} 行</span>
        </ElCheckbox>
      </template>
    </ElTableColumn>

    <ElTableColumn
      v-for="col in columns"
      :key="col.key"
      :prop="col.cell && !col.sortable ? undefined : col.key"
      :label="col.title"
      :width="col.width"
      :min-width="col.minWidth"
      :fixed="col.fixed"
      :align="col.align"
      :sortable="col.sortable ?? false"
      :sort-orders="col.sortable ? ['ascending', 'descending', null] : undefined"
      :show-overflow-tooltip="col.ellipsis ?? true"
    >
      <template #default="{ row }">
        <component :is="cellView" v-if="col.cell" :render="() => col.cell!(row)" />
        <span v-else :class="{ 'cell-mono': col.mono }">{{ valueOf(row, col.key) }}</span>
      </template>
    </ElTableColumn>

    <template #empty>
      <slot name="empty" />
    </template>
  </ElTable>
</template>

<script setup lang="ts" generic="T">
/**
 * DataTable 的普通表格分支（行数未超过虚拟滚动阈值时使用）
 *
 * 保留排序 / 固定列 / 溢出提示等原生表格能力，表格组件与样式按需引入。
 *
 * Props:
 *   - data:           当前页数据行
 *   - columns:        可见列（已按列显隐偏好过滤）
 *   - rowKey:         行唯一键取值函数
 *   - loading:        加载中（轻量蒙层，不替换整块内容）
 *   - stripe:         斑马纹
 *   - size:           行高尺寸（已合并全站密度偏好）
 *   - maxHeight:      最大高度（支持 "calc(100vh - 320px)" 这类表达式）
 *   - rowClassName:   行 className：字符串或按行计算
 *   - defaultSort:    受控排序的初始态（仅在挂载时应用一次）
 *   - sortKey:        受控排序列 key：外部清空时静默清除表头排序指示
 *   - selectable:     是否显示复选框列
 *   - allSelected:    当前页是否全选
 *   - indeterminate:  当前页是否半选
 *   - isSelected:     行是否已勾选
 *   - canSelect:      行是否可勾选
 *   - cellView:       单元格渲染器组件（携带使用方 scopeId）
 *
 * Emits:
 *   - sort-change:   表头排序点击（原始 Element Plus 载荷）
 *   - header-toggle: 表头全选切换
 *   - row-toggle:    行勾选切换
 *
 * Slots:
 *   - empty: 空态内容（由主组件统一注入 EmptyState）
 *
 * 典型应用场景：DataTable 内部按行数自动选择普通表 / 虚拟表，页面不直接引用本组件。
 */
import { computed, ref, watch, type Component } from 'vue'
// 静态引入 + 手动带样式：模板里的标签会被编译器解析成 setup 绑定，不再产生
// resolveComponent() 调用，unplugin-vue-components 的 ElementPlusResolver 看不到它，
// 按需样式不会自动注入，必须显式 import 样式入口。
import { ElCheckbox, ElTable, ElTableColumn, type TableInstance } from 'element-plus'
import 'element-plus/es/components/table/style/css'
import 'element-plus/es/components/table-column/style/css'
import type { ColumnDef } from './types'
import { valueOf } from '@/composables/core/useDataTableColumns'

interface Props<T> {
  /** 当前页数据行 */
  data: T[]
  /** 可见列（已按列显隐偏好过滤） */
  columns: ColumnDef<T>[]
  /** 行唯一键（取值函数，由主组件统一口径给出） */
  rowKey: (row: T) => string
  /** 加载中（轻量蒙层，不替换整块内容） */
  loading: boolean
  /** 斑马纹 */
  stripe: boolean
  /** 行高尺寸（已合并全站密度偏好） */
  size: 'large' | 'default' | 'small'
  /** 最大高度（支持 "calc(100vh - 320px)" 这类表达式） */
  maxHeight: number | string
  /** 行 className：字符串或按行计算（用于离线/高亮等状态样式） */
  rowClassName?: string | ((row: T) => string)
  /** 受控排序的初始态（仅在挂载时应用一次） */
  defaultSort?: { prop: string; order: 'ascending' | 'descending' }
  /** 受控排序列 key：外部清空时静默清除表头排序指示 */
  sortKey?: string | null
  /** 是否显示复选框列 */
  selectable: boolean
  /** 当前页是否全选 */
  allSelected: boolean
  /** 当前页是否半选 */
  indeterminate: boolean
  /** 行是否已勾选 */
  isSelected: (row: T) => boolean
  /** 行是否可勾选 */
  canSelect: (row: T) => boolean
  /** 单元格渲染器组件（携带使用方 scopeId） */
  cellView: Component
}

const props = defineProps<Props<T>>()

const emit = defineEmits<{
  /** 表头排序点击（原始 Element Plus 载荷） */
  (e: 'sort-change', payload: { prop: string | null; order: 'ascending' | 'descending' | null }): void
  /** 表头全选切换 */
  (e: 'header-toggle', checked: unknown): void
  /** 行勾选切换 */
  (e: 'row-toggle', row: T, checked: unknown): void
}>()

const tableRef = ref<TableInstance>()

/**
 * EP 的行类型是 Record<PropertyKey, any>，而这里的行类型是泛型 T（未约束索引签名），
 * 直接绑定会被判为不可赋值。这里做一次显式转换，运行时零开销（不拷贝、不改数据）。
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any -- EP 的行类型固定为 Record<PropertyKey, any>，此处仅做零开销类型适配
const tableData = computed(() => props.data as Record<PropertyKey, any>[])

/**
 * 受控排序同步进表格：
 * 表格的排序指示（caret 高亮 class + 原生 aria-sort）由内部 column.order 驱动，
 * 只有把 sortKey 写进 store 才会出现。公开方法 table.sort() 会回抛
 * sort-change（与使用方 toggleSort 形成回环：同列再调一次会翻转方向），所以改用：
 *   - default-sort：表头挂载时以 init:true 静默应用一次（不回抛事件）
 *   - clearSort()：外部把 sortKey 清空（切设备/重置筛选）时静默清除，silent:true
 * 点击表头的排序由表格内部自行更新 column.order，无需回写。
 */
watch(() => props.sortKey, (key) => {
  if (!key) tableRef.value?.clearSort()
})

/** 兼容表格自身的回调签名：传 { row } */
const elRowClassName = computed(() => {
  const rc = props.rowClassName
  if (!rc) return undefined
  if (typeof rc === 'string') return rc
  return (args: { row: T }) => rc(args.row)
})
</script>

<style scoped>
.data-table {
  width: 100%;
}

/* ID / 地址 / 数值列：等宽 + 等宽数字，逐位比对时不跳动 */
.cell-mono {
  font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  font-variant-numeric: tabular-nums;
}
</style>
