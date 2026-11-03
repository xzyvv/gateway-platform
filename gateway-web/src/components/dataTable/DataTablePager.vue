<template>
  <!--
    分页下沉：页面把 useTablePager 的字段直接接上即可，不再各自复制一套分页器。
    total 未传（不分页的表）或为 0 时不渲染，避免空表下面留一条"共 0 条"。
  -->
  <div class="table-pager">
    <ElPagination
      :current-page="page"
      :page-size="pageSize"
      :page-sizes="pageSizes"
      :total="total"
      layout="total, sizes, prev, pager, next, jumper"
      background
      @update:current-page="(v: number) => emit('update:page', v)"
      @update:page-size="(v: number) => emit('update:pageSize', v)"
    />
  </div>
</template>

<script setup lang="ts">
/**
 * DataTable 下沉的分页器
 *
 * 页面把 useTablePager 的字段直接接上即可，不再各自复制一套分页器。
 *
 * Props:
 *   - page:       当前页码
 *   - pageSize:   每页条数
 *   - pageSizes:  每页条数候选项
 *   - total:      总条数（未传表示不分页，不渲染页脚）
 *
 * Emits:
 *   - update:page:     当前页码变化
 *   - update:pageSize: 每页条数变化
 *
 * 典型应用场景：DataTable 内部在 total > 0 时渲染，页面不直接引用本组件。
 */
import { ElPagination } from 'element-plus'
// 分页器样式必须手动带：ElPagination 是从 element-plus 桶文件静态 import 的，
// 模板里的 <ElPagination> 会被编译器解析成 setup 绑定，不再产生 resolveComponent() 调用，
// unplugin-vue-components 的 ElementPlusResolver 因此看不到它，按需样式永不注入。
// 缺了这条时 .el-pagination 没有 display:flex，按钮竖排堆叠，1366×768 下分页器被整体推出首屏。
import 'element-plus/es/components/pagination/style/css'

interface Props {
  /** 当前页码 */
  page: number
  /** 每页条数 */
  pageSize: number
  /** 每页条数候选项 */
  pageSizes: number[]
  /** 总条数（未传表示不分页，不渲染页脚） */
  total?: number
}

defineProps<Props>()

const emit = defineEmits<{
  (e: 'update:page', page: number): void
  (e: 'update:pageSize', pageSize: number): void
}>()
</script>

<style scoped>
/* 分页固定在表格下方右侧：与表格同宽，避免页面各自复制一份布局样式 */
.table-pager {
  display: flex;
  justify-content: flex-end;
  padding-top: 12px;
}
</style>
