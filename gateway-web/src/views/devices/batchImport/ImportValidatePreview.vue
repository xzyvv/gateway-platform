<template>
  <el-alert
    :title="`共 ${total} 行，有效 ${valid} 行，无效 ${invalid} 行`"
    :type="invalid > 0 ? 'warning' : 'success'"
    :closable="false"
    show-icon
    style="margin-bottom: 12px"
  />
  <DataTable
    :data="rows"
    :columns="columns"
    row-key="line"
    stripe
    :loading="loading"
    :max-height="maxHeight"
    empty-text="暂无校验数据"
  />
</template>

<script setup lang="ts" generic="T extends object">
/**
 * 批量导入 · 共用「校验预览」区块。
 *
 * 设备分支与点位分支的校验预览形态一致：一条统计条（总数 / 有效 / 无效）
 * 加一张逐行明细表。行类型与列定义由调用方给出，本组件只负责渲染。
 *
 * Props:
 *   - rows:      逐行校验结果
 *   - columns:   明细表列定义（行类型由调用方与 rows 保持一致）
 *   - total:     总行数
 *   - valid:     有效行数
 *   - invalid:   无效行数
 *   - loading:   表格加载态
 *   - maxHeight: 表格最大高度（px）
 *
 * 典型应用场景：批量导入页的设备分支「校验预览」步与点位分支「校验预览」步。
 */
import DataTable, { type ColumnDef } from '@/components/DataTable.vue'

interface Props<T extends object> {
  rows: T[]
  columns: ColumnDef<T>[]
  total: number
  valid: number
  invalid: number
  loading: boolean
  maxHeight: number
}

defineProps<Props<T>>()
</script>

<style scoped>
/*
 * 校验错误信息：列定义的 cell 直接产出 class="error-text"，
 * 而单元格渲染器会带上「渲染表格的那个组件」的 scopeId，
 * 因此这条样式必须落在本组件（真正渲染 DataTable 的地方），不能留在页面根组件。
 */
.error-text { color: var(--text-danger); font-size: 0.75rem; }
</style>
