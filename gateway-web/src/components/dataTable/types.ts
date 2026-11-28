/**
 * 列定义类型已下沉到 `types/dataTable.ts`（DD-86 收尾）。
 *
 * 此处保留再导出，`DataTable.vue` 与既有 `@/components/DataTable.vue` 调用方不变。
 */
export type { ColumnDef } from '@/types/dataTable'
