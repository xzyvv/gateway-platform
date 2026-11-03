<template>
  <!--
    虚拟滚动分支：数据量超过阈值时启用，只渲染可见行。
    组件本体按需异步加载，常规表格页不必为虚拟表格承担额外体积。
  -->
  <div
    v-loading="loading"
    class="virtual-table-wrap"
    :style="{ height: `${height}px` }"
  >
    <ElAutoResizer>
      <template #default="{ height: measuredHeight, width }">
        <ElTableV2
          :columns="columns"
          :data="data"
          :width="width"
          :height="Math.max(300, Math.min(measuredHeight, height))"
          :row-key="rowKey"
          :row-class="rowClass"
          :estimated-row-height="estimatedRowHeight"
          fixed
          :empty-text="emptyText"
        >
          <template #empty>
            <slot name="empty" />
          </template>
        </ElTableV2>
      </template>
    </ElAutoResizer>
  </div>
</template>

<script setup lang="ts" generic="T">
/**
 * DataTable 的虚拟滚动分支（数据量超过阈值时启用）
 *
 * 组件本体与样式按需异步加载，常规表格页不必为虚拟表格承担额外体积。
 *
 * Props:
 *   - columns:             归一化后的虚拟表格列定义（固定像素宽 + 自绘表头/单元格渲染器）
 *   - data:                虚拟表格数据源（函数型 rowKey 时已注入不可枚举键字段）
 *   - rowKey:              虚拟表格的 rowKey（只接受字段名）
 *   - rowClassName:        行 className：字符串或按行计算
 *   - estimatedRowHeight:  预估行高（px）
 *   - height:              容器高度（px）
 *   - emptyText:           空态文案
 *   - loading:             加载中（轻量蒙层）
 *
 * Slots:
 *   - empty: 空态内容（由主组件统一注入 EmptyState）
 *
 * 典型应用场景：DataTable 内部按行数自动选择普通表 / 虚拟表，页面不直接引用本组件。
 */
import { computed, defineAsyncComponent } from 'vue'
import type { Column } from 'element-plus'

interface Props<T> {
  /** 归一化后的虚拟表格列定义（固定像素宽 + 自绘表头/单元格渲染器） */
  columns: Column[]
  /** 虚拟表格数据源（函数型 rowKey 时已注入不可枚举键字段） */
  data: T[]
  /** 虚拟表格的 rowKey（只接受字段名） */
  rowKey: string
  /** 行 className：字符串或按行计算（用于离线/高亮等状态样式） */
  rowClassName?: string | ((row: T) => string)
  /** 预估行高（px） */
  estimatedRowHeight: number
  /** 容器高度（px） */
  height: number
  /** 空态文案 */
  emptyText: string
  /** 加载中（轻量蒙层） */
  loading: boolean
}

const props = defineProps<Props<T>>()

/**
 * 虚拟表格只在数据量超过阈值时启用，按需异步加载：
 * 常规表格页不必为虚拟表格承担约 48KB(gzip) 的额外体积。
 *
 * 样式必须手动带上：这里是动态 import，不经过 unplugin-vue-components 的
 * ElementPlusResolver，resolver 不会为它注入按需样式。缺了 style/css 时虚拟表格
 * 没有任何布局规则（单元格纵向堆叠、表头落到末尾、无边框），行数一过阈值就散架。
 * style/css 是官方样式入口（base + virtual-list + empty + 虚拟表格，约 6.4KB），
 * 与组件本体一起按需加载，不影响常规表格页体积。
 */
const loadTableV2 = () => import('element-plus/es/components/table-v2/index.mjs')
const loadTableV2Style = () => import('element-plus/es/components/table-v2/style/css')

const ElTableV2 = defineAsyncComponent(() => Promise.all([loadTableV2(), loadTableV2Style()]).then(([m]) => m.ElTableV2))
const ElAutoResizer = defineAsyncComponent(() => Promise.all([loadTableV2(), loadTableV2Style()]).then(([m]) => m.ElAutoResizer))

/** 兼容虚拟表格自身的回调签名：传 { rowData } */
const rowClass = computed(() => {
  const rc = props.rowClassName
  if (!rc) return undefined
  if (typeof rc === 'string') return rc
  return (args: { rowData: T; rowIndex: number }) => rc(args.rowData)
})
</script>

<style scoped>
/* 虚拟滚动容器：高度由 auto-resizer 测量后回传（避免每行 DOM 全量渲染） */
.virtual-table-wrap {
  width: 100%;
  max-height: 60vh;
}
</style>
