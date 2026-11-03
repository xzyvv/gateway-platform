<template>
  <div
    class="data-table-wrap"
    ref="wrapRef"
    :class="{ 'is-h-overflow': hOverflow, 'is-h-end': hAtEnd }"
  >
    <!--
      批量操作条下沉到表格：勾选态由表格自己维护，页面不再各拼一遍 SelectionBar。
      只有提供了 batch-actions 插槽才渲染，避免只要复选框的页面被塞一条空操作条。
    -->
    <SelectionBar
      v-if="selectable && selectedRows.length > 0 && $slots['batch-actions']"
      :count="selectedRows.length"
      :hint="batchHint"
      @clear="clearSelection"
    >
      <slot name="batch-actions" :rows="selectedRows" />
    </SelectionBar>

    <!-- 错误态下沉到表格：错误条 + 重试按钮由表格统一渲染，页面只传文案 -->
    <DataTableErrorBar
      v-if="error && !loading"
      :message="error"
      @retry="emit('retry')"
    />

    <!-- 首次加载且没有任何行：用骨架屏替代转圈，提前画出表格结构，数据到达时布局不跳动 -->
    <TableSkeleton
      v-if="showSkeleton"
      :rows="8"
      :columns="skeletonColumns"
      :dense="effectiveSize === 'small'"
    />

    <!-- 数据量超过阈值时切换虚拟滚动（el-table-v2），只渲染可见行 -->
    <DataTableVirtual
      v-else-if="useVirtual"
      :columns="v2Columns"
      :data="v2Data"
      :row-key="v2RowKey"
      :row-class-name="rowClassName"
      :estimated-row-height="estimatedRowHeight"
      :height="effectiveHeight"
      :empty-text="emptyText"
      :loading="loading"
    >
      <template #empty>
        <slot name="empty">
          <EmptyState
            v-if="!error"
            :description="emptyText"
            :image-size="80"
            :action-text="emptyActionText"
            @action="emit('empty-action')"
          />
        </slot>
      </template>
    </DataTableVirtual>

    <!-- 小数据量走普通表格，保留排序/固定列/溢出提示等原生能力 -->
    <DataTablePlain
      v-else
      :data="data"
      :columns="visibleColumns"
      :row-key="keyOf"
      :loading="loading"
      :stripe="stripe"
      :size="effectiveSize"
      :max-height="maxHeight"
      :row-class-name="rowClassName"
      :default-sort="elDefaultSort"
      :sort-key="sortKey"
      :selectable="selectable"
      :all-selected="allSelected"
      :indeterminate="indeterminate"
      :is-selected="isSelected"
      :can-select="canSelect"
      :cell-view="cellView"
      @header-toggle="onHeaderToggle"
      @row-toggle="onRowToggle"
      @sort-change="onSortChange"
    >
      <template #empty>
        <slot name="empty">
          <EmptyState
            v-if="!error"
            :description="emptyText"
            :image-size="80"
            :action-text="emptyActionText"
            @action="emit('empty-action')"
          />
        </slot>
      </template>
    </DataTablePlain>

    <!--
      分页下沉：页面把 useTablePager 的字段直接接上即可，不再各自复制一套分页器。
      total 未传（不分页的表）或为 0 时不渲染，避免空表下面留一条"共 0 条"。
    -->
    <DataTablePager
      v-if="pagerVisible"
      :page="page"
      :page-size="pageSize"
      :page-sizes="pageSizes"
      :total="total"
      @update:page="(v: number) => emit('update:page', v)"
      @update:page-size="(v: number) => emit('update:pageSize', v)"
    />
  </div>
</template>

<script setup lang="ts" generic="T">
import { computed, getCurrentInstance, ref } from 'vue'
import EmptyState from './EmptyState.vue'
import SelectionBar from './SelectionBar.vue'
import TableSkeleton from './TableSkeleton.vue'
import DataTableErrorBar from './dataTable/DataTableErrorBar.vue'
import DataTablePager from './dataTable/DataTablePager.vue'
import DataTablePlain from './dataTable/DataTablePlain.vue'
import DataTableVirtual from './dataTable/DataTableVirtual.vue'
import { createCellView } from './dataTable/cellView'
import { useTableDensity } from '@/composables/core/useTablePrefs'
import { useDataTableA11y } from '@/composables/core/useDataTableA11y'
import { useDataTableColumns } from '@/composables/core/useDataTableColumns'
import { useDataTableOverflow } from '@/composables/core/useDataTableOverflow'
import { useDataTableSelection } from '@/composables/core/useDataTableSelection'
import { useDataTableVirtual } from '@/composables/core/useDataTableVirtual'
import type { ColumnDef } from './dataTable/types'

/** 列定义类型对外仍从这里导出，页面导入路径不变 */
export type { ColumnDef }

interface Props<T> {
  data: T[]
  columns: ColumnDef<T>[]
  /** 行唯一键：字段名或取值函数（选中态与虚拟滚动都依赖它） */
  rowKey: string | ((row: T) => string | number)
  /** true 强制虚拟，false 强制普通，'auto' 按行数自动选择（默认） */
  virtual?: boolean | 'auto'
  /** 虚拟滚动容器高度（px） */
  height?: number
  estimatedRowHeight?: number
  emptyText?: string
  loading?: boolean
  /**
   * 加载失败时的错误文案（传空表示正常）。
   *
   * 页面加载失败若只弹 toast，表格会退化成空态「暂无数据」，
   * 用户会误判成「网关没有这条数据」而不是「接口失败」。
   * 传进来由表格统一渲染错误条 + 重试按钮。
   */
  error?: string
  /** 启用复选框列，选中变化通过 selection-change 抛出整行对象 */
  selectable?: boolean
  /** 行级勾选禁用（例如只读行不参与批量操作）；未提供时全部可选 */
  rowSelectable?: (row: T) => boolean
  /** 翻页/换页时保留勾选（配合"跨页批量操作"）；为 false 时数据源一变就清空 */
  preserveSelection?: boolean
  /** 需要隐藏的列 key：由页面把"列显隐"偏好传进来，组件本身不持久化 */
  hiddenColumns?: string[]
  /** 受控排序：当前排序列 key（未排序传 null）。排序结果由使用方（如 useTablePager）负责，组件只渲染状态 */
  sortKey?: string | null
  sortOrder?: 'asc' | 'desc'
  /** el-table 专用：斑马纹 */
  stripe?: boolean
  /** 行高尺寸；不传则跟随全站表格密度偏好（useTableDensity），由用户在表格显示设置里切换 */
  size?: 'large' | 'default' | 'small'
  /** 行 className：字符串或按行计算（用于离线/高亮等状态样式） */
  rowClassName?: string | ((row: T) => string)
  /**
   * 最大高度：普通表直接交给 el-table 的 max-height（支持 "calc(100vh - 320px)" 这类表达式），
   * 虚拟表需要数值，传字符串时会被忽略并退回默认高度。
   */
  maxHeight?: number | string
  /** 批量操作条提示语（跨页勾选等）；仅提供 batch-actions 插槽时可见 */
  batchHint?: string
  /** 空态主行动文案；不传则只显示描述 */
  emptyActionText?: string
  /**
   * 表格的无障碍名称（读屏器播报的表格标题）。
   *
   * 不传时由列名自动拼装（如「设备 ID、设备名、最后成功采集…共 10 列」），
   * 保证每张表都有可访问名称——表格没有可访问名称时，读屏器只会念
   * 「表格」两个字，用户完全不知道这张表装的是什么数据。
   */
  caption?: string
  /** 分页：total 传了才渲染页脚（配合 useTablePager 的 page / pageSize） */
  page?: number
  pageSize?: number
  total?: number
  pageSizes?: number[]
}

const props = withDefaults(defineProps<Props<T>>(), {
  virtual: 'auto',
  height: 480,
  estimatedRowHeight: 44,
  emptyText: '暂无数据',
  loading: false,
  error: '',
  selectable: false,
  rowSelectable: undefined,
  preserveSelection: false,
  hiddenColumns: () => [],
  sortKey: null,
  sortOrder: 'asc',
  stripe: false,
  // 默认固定表头：长列表滚动时列名不跟着滚走，无需每页各自声明。
  // 用视口比例而非固定 560px：笔记本 1366×768 下固定 560 会把分页器顶到 863px，
  // 用户必须先滚一屏才能翻页。保留一个下限，避免大屏上表格被压得过矮。
  maxHeight: 'max(360px, calc(100vh - 320px))',
  batchHint: '',
  emptyActionText: '',
  page: 1,
  pageSize: 100,
  pageSizes: () => [50, 100, 200, 500],
})

const emit = defineEmits<{
  (e: 'selection-change', rows: T[]): void
  /** 表头排序点击；排序本身由使用方完成（组件只拿到当页数据，页内排序会跨页错乱） */
  (e: 'sort-change', payload: { key: string; order: 'asc' | 'desc' }): void
  (e: 'update:page', page: number): void
  (e: 'update:pageSize', pageSize: number): void
  /** 空态主行动点击（如「清空搜索」「立即新增」），由页面决定跳转/重置 */
  (e: 'empty-action'): void
  /** 点击错误条上的「重试」，由页面重新拉数据 */
  (e: 'retry'): void
}>()

/**
 * 行高尺寸：显式传入优先，否则跟随全站密度偏好。
 * 密度是跨表格的同一件事（笔记本上想一屏多看几行），让 39 处页面各自写死
 * size="small" 会让偏好切换形同虚设。
 */
const { density } = useTableDensity()
const effectiveSize = computed(() => props.size ?? (density.value === 'compact' ? 'small' : 'default'))

/**
 * 单元格内容由使用方的 `cell` 函数产出，必须带上使用方组件的 scopeId，
 * 否则使用方 scoped 样式（.value-cell / .comms-cell 等）匹配不到单元格内节点
 * （`h()` 创建 vnode 时的 scopeId 取自「当前渲染实例」，而 cell 在表格组件实例内执行）。
 * 这里把使用方 vnode 上的 scopeId 注入 CellView，等价 SFC 编译器注入的 `__scopeId`。
 */
const hostScopeId = getCurrentInstance()?.vnode.scopeId ?? undefined
/** 单元格渲染器：普通表格分支与虚拟滚动分支共用同一个（都需继承使用方 scopeId） */
const cellView = createCellView(hostScopeId)

/** 可见列 / 表格无障碍名称 / 骨架屏列数 */
const { visibleColumns, tableCaption, skeletonColumns } = useDataTableColumns<T>({
  getColumns: () => props.columns,
  getHiddenColumns: () => props.hiddenColumns ?? [],
  getCaption: () => props.caption ?? '',
  getSelectable: () => props.selectable,
})

/** 勾选态（普通表格分支与虚拟滚动分支共用同一份状态） */
const {
  selectedRows,
  selectedIndex,
  keyOf,
  canSelect,
  isSelected,
  allSelected,
  indeterminate,
  onHeaderToggle,
  onRowToggle,
  clearSelection,
} = useDataTableSelection<T>({
  getData: () => props.data,
  getRowKey: () => props.rowKey,
  getRowSelectable: () => props.rowSelectable,
  getSelectable: () => props.selectable,
  getPreserveSelection: () => props.preserveSelection,
  onChange: (rows) => emit('selection-change', rows),
})

/**
 * 内部用 'asc'/'desc'，el-table 用 'ascending'/'descending'（且未排序为 null）。
 * 转换收口在这里，避免模板里写两套字面量。
 */
function elOrderOf(order: 'asc' | 'desc'): 'ascending' | 'descending' {
  return order === 'asc' ? 'ascending' : 'descending'
}

/**
 * 受控排序同步进 el-table：
 * el-table 的排序指示（caret 高亮 class + 原生 aria-sort）由内部 column.order 驱动，
 * 只有把 sortKey/sortOrder 写进 store 才会出现。公开方法 table.sort() 会回抛
 * sort-change（与使用方 toggleSort 形成回环：同列再调一次会翻转方向），所以改用：
 *   - default-sort：table-header 挂载时以 init:true 静默应用一次（不回抛事件）
 *   - clearSort()：外部把 sortKey 清空（切设备/重置筛选）时静默清除，silent:true
 * 点击表头的排序由 el-table 内部自行更新 column.order，无需回写。
 * 注意：sortable 列即使带自定义 cell 也必须带 prop，否则 commit('sort') 按
 * column.property 匹配不到列，指示器静默丢失（DeviceList 的分组列踩过这个坑）。
 */
const elDefaultSort = computed(() =>
  props.sortKey ? { prop: props.sortKey, order: elOrderOf(props.sortOrder) } : undefined,
)

/**
 * 表头点击 → 向上抛 sort-change。
 * 组件自己不去排序：它只拿到当前页数据，页内排序会让"第 2 页比第 1 页小"。
 * 三段循环（升 → 降 → 取消）里取消态直接抛 null key，由使用方复位。
 */
function onSortChange(payload: { prop: string | null; order: 'ascending' | 'descending' | null }) {
  if (!payload.prop || !payload.order) {
    emit('sort-change', { key: '', order: 'asc' })
    return
  }
  emit('sort-change', { key: payload.prop, order: payload.order === 'ascending' ? 'asc' : 'desc' })
}

/** v2 表头点击/键盘共用同一套排序翻转：未排序→升序，升序→降序（与既有点击行为保持一致） */
function requestSort(col: ColumnDef<T>) {
  const active = props.sortKey === col.key
  const order = active ? props.sortOrder : 'asc'
  emit('sort-change', { key: col.key, order: active && order === 'asc' ? 'desc' : order })
}

/** 虚拟滚动分支的全部计算（引擎选择 / 高度 / 列定义 / 数据源） */
const { useVirtual, effectiveHeight, v2RowKey, v2Data, v2Columns } = useDataTableVirtual<T>({
  getData: () => props.data,
  getRowKey: () => props.rowKey,
  getVirtualMode: () => props.virtual,
  getHeight: () => props.height,
  getMaxHeight: () => props.maxHeight,
  getVisibleColumns: () => visibleColumns.value,
  getSelectable: () => props.selectable,
  getSortKey: () => props.sortKey,
  getSortOrder: () => props.sortOrder,
  requestSort,
  cellView,
  selection: {
    allSelected: () => allSelected.value,
    indeterminate: () => indeterminate.value,
    isSelected,
    canSelect,
    onHeaderToggle,
    onRowToggle,
  },
})

/** 表格根容器：横向溢出提示与无障碍补丁都以此为查询范围 */
const wrapRef = ref<HTMLElement>()

/** 无障碍补丁：aria-sort 校准、表格名称、虚拟滚动表格的表格语义兜底 */
useDataTableA11y({
  wrapRef,
  getCaption: () => tableCaption.value,
  getRowCount: () => props.data.length,
  useVirtual,
})

/** 横向溢出提示（右缘渐隐阴影） */
const { hOverflow, hAtEnd } = useDataTableOverflow({ wrapRef })

/**
 * 仅「首次加载且一行都没有」时用骨架屏：
 * 表格已有数据时刷新仍走轻量蒙层，否则整块内容消失又重排，比转圈更刺眼。
 */
const showSkeleton = computed(() => props.loading && props.data.length === 0)

/** total 未传（不分页）或为 0 时不渲染页脚：空表下方再挂一条"共 0 条"只是噪音 */
const pagerVisible = computed(() => props.total !== undefined && props.total > 0)

defineExpose({ clearSelection, selectedRows, selectedIndex })
</script>

<style scoped>
.data-table-wrap {
  width: 100%;
}
</style>
