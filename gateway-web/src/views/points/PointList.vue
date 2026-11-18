<template>
  <div class="points">
    <el-card shadow="never" class="section-card">
      <template #header>
        <!-- 页面标题由 HubShell 按页签渲染（点位清单），这里只保留数量徽标与操作区 -->
        <div class="list-toolbar">
          <el-tag type="info" effect="plain" round size="small" class="count-badge">
            {{ keyword.trim() ? `${total} / ${mergedPoints.length} 个` : `${mergedPoints.length} 个` }}
          </el-tag>
          <div class="list-actions">
            <el-select
              v-model="selectedDevice"
              :placeholder="devices.length === 0 ? '暂无可选设备' : '选择设备'"
              aria-label="选择设备"
              style="width: 200px"
              @change="onDeviceChange"
              clearable
              filterable
              :disabled="devices.length === 0"
            >
              <el-option v-for="d in devices" :key="d.id" :label="d.name" :value="d.id" />
            </el-select>
            <el-input
              v-model="keyword"
              class="search-input"
              placeholder="搜索 ID / 名称 / 地址"
              aria-label="搜索点位"
              clearable
              :disabled="!selectedDevice"
            >
              <template #prefix><el-icon><Search /></el-icon></template>
            </el-input>
            <el-tooltip content="刷新点位列表" placement="bottom">
              <el-button size="small" :aria-label="'刷新'" @click="loadPoints"><el-icon><Refresh /></el-icon></el-button>
            </el-tooltip>
            <el-tooltip :disabled="!!selectedDevice" content="请先选择设备" placement="bottom">
              <span>
                <el-button v-if="canWritePoint" type="primary" size="small" :disabled="!selectedDevice" @click="openCreate"><el-icon><Plus /></el-icon> 新增点位</el-button>
              </span>
            </el-tooltip>
            <!-- 批量导入统一收口到「CSV / 粘贴 JSON」双模式页面，这里只做入口 -->
            <el-tooltip content="CSV 文件或粘贴 JSON 批量导入" placement="bottom">
              <span>
                <el-button v-if="canWritePoint" size="small" @click="router.push('/devices?tab=batch-import')"><el-icon><Upload /></el-icon> 批量导入</el-button>
              </span>
            </el-tooltip>
            <!-- 导出是只读操作，不受 point.write 限制；CSV 可直接改完走批量导入 -->
            <el-tooltip content="导出全部配置点位（模板设备展开出的点位不在导出范围）" placement="bottom">
              <span>
                <el-dropdown trigger="click" :disabled="exportingPoints" @command="doExportPoints">
                  <el-button size="small" :loading="exportingPoints"><el-icon><Download /></el-icon> 批量导出</el-button>
                  <template #dropdown>
                    <el-dropdown-menu>
                      <el-dropdown-item command="csv-basic">导出 CSV（精简 10 列）</el-dropdown-item>
                      <el-dropdown-item command="csv-full">导出 CSV（完整字段）</el-dropdown-item>
                      <el-dropdown-item command="json">导出 JSON（完整快照，可粘贴导入）</el-dropdown-item>
                    </el-dropdown-menu>
                  </template>
                </el-dropdown>
              </span>
            </el-tooltip>
            <!-- 低频操作收进"更多"，避免主操作被同权重按钮淹没 -->
            <el-dropdown v-if="canWritePoint" trigger="click" :disabled="!selectedDevice" @command="onMoreCommand">
              <el-button size="small" :disabled="!selectedDevice">
                更多操作<el-icon class="el-icon--right"><ArrowDown /></el-icon>
              </el-button>
              <template #dropdown>
                <el-dropdown-menu>
                  <el-dropdown-item command="copy"><el-icon><CopyDocument /></el-icon> 复制到其他设备</el-dropdown-item>
                  <el-dropdown-item command="generate"><el-icon><MagicStick /></el-icon> 批量生成</el-dropdown-item>
                </el-dropdown-menu>
              </template>
            </el-dropdown>
            <!-- 表格显示设置：行密度全站共享，列显隐按点位表记忆 -->
            <TablePrefsMenu table-id="points" :columns="toggleableColumns" />
          </div>
        </div>
      </template>

      <!-- 加载失败必须显式说明：否则空表会被当成「该设备没有点位」 -->
      <el-alert
        v-if="loadError"
        type="error"
        :closable="false"
        show-icon
        class="load-error"
        :title="`点位数据加载失败：${loadError}`"
      >
        <el-button size="small" type="danger" plain @click="retryLoad">重试</el-button>
      </el-alert>

      <!--
        批量操作条与分页都已下沉到 DataTable：
        页面只声明「对选中项能做什么」，不再重复拼 SelectionBar / el-pagination。
      -->
      <DataTable
        ref="tableRef"
        :data="pagedPoints"
        :columns="pointColumns"
        :hidden-columns="hiddenColumns"
        :sort-key="sortKey"
        :sort-order="sortOrder"
        row-key="id"
        selectable
        :row-selectable="isRowSelectable"
        preserve-selection
        stripe
        :loading="loading"
        :empty-text="emptyDescription"
        :empty-action-text="emptyAction?.text"
        :row-class-name="pointRowClass"
        :batch-hint="pageCount > 1 ? '可跨页勾选，操作作用于全部已选' : ''"
        v-model:page="page"
        v-model:page-size="pageSize"
        :total="total"
        :page-sizes="pageSizes"
        @selection-change="onSelectionChange"
        @sort-change="onSortChange"
        @empty-action="emptyAction?.run()"
      >
        <template #batch-actions>
          <el-button v-if="canWritePoint" type="warning" size="small" @click="showBatchEdit = true">
            <el-icon><Operation /></el-icon> 批量修改
          </el-button>
          <el-button v-if="canWritePoint" type="danger" size="small" @click="batchDelete">
            <el-icon><Delete /></el-icon> 批量删除
          </el-button>
        </template>
      </DataTable>
    </el-card>

    <PointsCopyDialog v-model="showCopy" :devices="devices" :selected-device="selectedDevice" :selected-device-name="currentDeviceName" :points="points" @copied="loadPoints" />

    <!-- 单点新增/编辑弹窗：表单模型与回调由本页 provide，弹窗侧 inject -->
    <PointsFormDialog />

    <PointsBatchEditDialog v-model="showBatchEdit" :selection="selection" :selected-device="selectedDevice" @edited="loadPoints" />
    <PointsGenerateDialog v-model="showGen" :device-id="selectedDevice" :device-name="currentDeviceName" :devices="devices" @generated="loadPoints" />
  </div>
</template>
<script setup lang="ts">
import { computed, nextTick, provide, ref, watch } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import { ArrowDown, CopyDocument, Download, MagicStick, Refresh, Search, Upload, Plus } from '@element-plus/icons-vue'
import { usePoints } from '@/composables/devices/usePoints'
import { useTablePager } from '@/composables/core/useTablePager'
import { useQueryState } from '@/composables/core/useQueryState'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import { usePermission } from '@/composables/auth/usePermission'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
// 必须显式 import：这些文件在 src/views/points/dialogs/ 下，而 unplugin-vue-components
// 默认只扫 src/components/，扫不到就只在控制台留一条 Failed to resolve component，
// 按钮点得动但弹窗永远不出现（静默失效）。
import PointsCopyDialog from './dialogs/PointsCopyDialog.vue'
import PointsFormDialog from './dialogs/PointsFormDialog.vue'
import PointsBatchEditDialog from './dialogs/PointsBatchEditDialog.vue'
import PointsGenerateDialog from './dialogs/PointsGenerateDialog.vue'
import { pointFormContextKey } from './pointFormContext'
import { createPointColumns } from './parts/pointColumns'
import type { PointRow } from '@/composables/devices/usePoints'
import { flashHighlightRow } from '@/utils/tableHighlight'
import { useUnmountedGuard } from '@/composables/core/useUnmountedGuard'

const router = useRouter()
const route = useRoute()
function goDevices() { router.push("/devices") }

const {
  devices, points, mergedPoints, loading, loadError, selectedDevice, currentDeviceName,
  northInterval, selection,
  pointForm, pointFormRef, pointRules, formMode, submitting, showForm,
  showCopy, showBatchEdit, showGen,
  exportingPoints, doExportPoints,
  protocolLabel,
  onSelectionChange, loadPoints, loadDevices,
  readPoint, removePoint, batchDelete,
  openCreate, openEdit, resetForm, submitPoint, toggleWritable, savePointField,
} = usePoints()

// 按钮级权限：写操作（增删改 / 批量导入 / 批量生成）仅 point.write 可见
const { can } = usePermission()
const canWritePoint = computed(() => can('point.write'))

// 未保存更改守卫：新增/编辑点位弹窗。弹窗级拦 X/ESC/取消；scope 桥接路由级注册表，
// 弹窗开着跳路由 / 切 Hub tab 仍被 router.beforeEach / HubShell 拦截。
const formGuard = useDialogDirtyGuard(showForm, () => pointForm, { scope: route.path })

// 单点表单弹窗（PointsFormDialog.vue）的上下文：表单模型是 reactive 对象，
// 弹窗模板里的 v-model 会直接改它的字段——走 props 下发会被 vue/no-mutating-props
// 拦下，注入同一个对象则与拆分前行为完全一致（详见 pointFormContext.ts）。
provide(pointFormContextKey, {
  pointForm, pointRules, pointFormRef, formMode, submitting, showForm,
  currentDeviceName, northInterval, protocolLabel, formGuard,
  submitPoint, resetForm,
})

/**
 * 点位列表的搜索 + 分页。
 * 点位是单设备可上千行的数据，只靠滚动很难定位；分页后配合 DataTable 的
 * preserve-selection，可以「本页全选 → 翻页继续选 → 一次性批量删除」。
 */
const keyword = useQueryState('kw')
// 设备选择写进 URL：切到别的页面再回来、或刷新后仍停在原来的设备上
const deviceParam = useQueryState('device')
// 深链带过来的目标点位 ID（?highlight=）：高亮 + 翻页定位 + 滚动到视口中央
const highlight = useQueryState('highlight')
const tableRef = ref<{ clearSelection: () => void } | null>(null)

const kw = computed(() => keyword.value.trim().toLowerCase())
const {
  page, pageSize, pageSizes, total, pageCount, matched, paged: pagedPoints, reset,
  sortKey, sortOrder, toggleSort,
} = useTablePager(
  mergedPoints,
  (p) =>
    !kw.value ||
    [p.id, p.name, p.address, p.bus, String(p.slave_id ?? '')]
      .some((v) => String(v ?? '').toLowerCase().includes(kw.value)),
  // 页码/每页行数/排序进 URL：现场出问题时可直接把这一屏的链接发过来
  { syncUrl: true },
)

/** 模板展开点位只读展示：不参与勾选（批量删除/批量修改只对配置点位生效） */
function isRowSelectable(row: PointRow): boolean {
  return row._source !== 'template'
}

/**
 * 表头排序：从站地址升序（1 号排在最前）符合现场按地址排查的习惯；
 * 其余列默认升序。DataTable 只抛事件不自己排序，排序在分页入口完成。
 */
function onSortChange({ key, order }: { key: string; order: 'asc' | 'desc' }) {
  // 三段循环的第三段（取消排序）传空 key：复位回数据源原始顺序
  if (!key) {
    sortKey.value = null
    sortOrder.value = 'asc'
    reset()
    return
  }
  toggleSort(key, order)
}

const emptyDescription = computed(() => {
  // 失败时不能落到「暂无点位」：那会被当成设备里没有点位
  if (loadError.value) return '点位数据加载失败，请点击上方「重试」'
  if (kw.value && !loading.value) return `没有匹配「${keyword.value.trim()}」的点位`
  if (devices.value.length === 0) return '请先到设备管理中新增设备'
  // 清单合并展示配置点位与模板展开的运行时点位；两者皆空才是真正的「没有点位」
  return selectedDevice.value
    ? '该设备没有运行中的点位（含模板展开点位），可点「新增点位」单独添加'
    : '请先选择设备'
})

/** 失败重试：设备列表都没拿到时先补设备，设备已就绪就只重拉点位 */
async function retryLoad() {
  if (devices.value.length === 0) await loadDevices()
  if (selectedDevice.value) loadPoints()
}

// 空态的下一步动作：搜索没命中→清空搜索；没有设备→去设备管理；其余情况无动作
const emptyAction = computed<{ text: string; run: () => void } | null>(() => {
  // 加载失败时「前往设备管理」是错的下一步，失败态由上方 alert 的重试按钮承担
  if (loadError.value) return null
  if (kw.value && !loading.value) return { text: '清空搜索', run: () => (keyword.value = '') }
  if (devices.value.length === 0) return { text: '前往设备管理', run: goDevices }
  return null
})

// 关键词变化回到第一页，避免搜索后停在空白页
watch(keyword, reset)

// 刷新/切换设备/增删改后数据源整体替换：页码复位并把勾选清空，
// 否则旧勾选会继续参与批量删除，删到已不存在的点位。
watch(points, () => {
  reset()
  tableRef.value?.clearSelection()
  // 深链带 highlight：数据就绪后翻页 + 滚动定位到目标行
  if (highlight.value) navigateToHighlightedPoint()
})

function onDeviceChange() {
  keyword.value = ''
  deviceParam.value = selectedDevice.value
  loadPoints()
}

/**
 * 行高亮：深链跳转过来时，目标点位行加 is-highlighted 类
 * （蓝左边 + 浅蓝背景，与设备列表一致）。
 */
function pointRowClass({ row }: { row: PointRow }): string {
  return highlight.value && row.id === highlight.value ? 'is-highlighted' : ''
}

/**
 * 滚动到高亮行并闪烁提示（与设备清单共用同一实现）。
 *
 * 定时器在组件卸载时自动作废——不会往已销毁的 DOM 上写 class。
 */
const { isUnmounted } = useUnmountedGuard()
const scrollToHighlightedRow = flashHighlightRow('.points', () => isUnmounted())

/**
 * 跳转到目标点位所在页并滚动到视口中央。
 *
 * 点位是分页的，目标点可能不在当前页：先在已筛选全量里找到索引，
 * 算出页码并切过去；等 DOM 更新后再 scrollIntoView + 闪烁强调。
 */
function navigateToHighlightedPoint() {
  const hid = highlight.value
  if (!hid) return
  // 在已匹配（关键词筛选后）的全量点位里定位
  const idx = matched.value.findIndex((p) => p.id === hid)
  if (idx < 0) return
  const targetPage = Math.floor(idx / pageSize.value) + 1
  if (page.value !== targetPage) {
    page.value = targetPage
  }
  // 翻页后表格重新渲染，等下一帧再滚动
  nextTick(() => {
    scrollToHighlightedRow()
  })
}

/**
 * URL 里指定的设备优先于 composable 自动选中的第一台：
 * 从链接/刷新回来时要停在用户上次看的设备，而不是被重置。
 * devices 异步加载完成后才能校验 id 是否存在。
 */
watch(devices, (list) => {
  const wanted = deviceParam.value
  if (!wanted || selectedDevice.value === wanted) return
  if (!list.some((d) => d.id === wanted)) return
  selectedDevice.value = wanted
  loadPoints()
}, { immediate: true })

/** 更多操作菜单：低频动作收进来，保持工具栏主次分明 */
function onMoreCommand(cmd: string | number | object) {
  if (cmd === 'copy') showCopy.value = true
  else if (cmd === 'generate') showGen.value = true
}

/**
 * 表格显示偏好：列显隐（按点位表记忆）+ 行密度（全站共享，见 useTablePrefs）。
 * 1366×768 的现场笔记本上，9 列点位表必然要横向滚动；允许隐藏无关列，
 * 偏好写本地存储，下次进入沿用而不是每次都重设。
 */
const toggleableColumns = [
  { key: '_source', title: '来源' },
  { key: 'name', title: '点位名称' },
  { key: 'bus', title: '总线' },
  { key: 'protocol', title: '协议' },
  { key: 'address', title: '点位地址' },
  { key: 'slave_id', title: '从站地址' },
  { key: 'ratio', title: '变比' },
  { key: 'writable', title: '允许写入' },
]
const { hidden: hiddenColumns } = useColumnPrefs('points', {
  // 1366 笔记本上 10 列点位表必然横向滚动：默认收起两列「每行都一样/只是溯源」的信息，
  // 「从站地址」「操作」等排查与操作必需的列始终可见（用户可在显示设置里勾回来）。
  narrowHidden: ['_source', 'bus'],
})

/**
 * 点位列定义：定义本身在 parts/pointColumns.ts（145 行），这里只注入页面动作。
 */
const pointColumns = createPointColumns({
  canWrite: () => canWritePoint.value,
  readPoint,
  openEdit,
  removePoint,
  toggleWritable,
  savePointField,
})
</script>

<style scoped src="./PointList.css"></style>
