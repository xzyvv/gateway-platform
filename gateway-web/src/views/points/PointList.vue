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

    <el-dialog v-model="showForm" :title="formMode === 'create' ? '新增点位' : '编辑点位'" width="620px" :before-close="formGuard.beforeClose" @close="resetForm">
      <el-form ref="pointFormRef" :model="pointForm" :rules="pointRules" label-width="90px">
        <el-form-item label="点位 ID" prop="id">
          <el-input v-model="pointForm.id" :disabled="formMode === 'edit'" placeholder="如: temp_01" />
        </el-form-item>
        <el-form-item label="名称" prop="name">
          <el-input v-model="pointForm.name" placeholder="如: 温度传感器" />
        </el-form-item>
        <el-form-item label="总线" prop="bus">
          <!-- N3: 总线必须继承所属设备。原自由文本可填任意总线名，后端按 bus 找不到采集器，
               该点位永远采不到数据且界面无任何提示。 -->
          <el-input :model-value="pointForm.bus" disabled placeholder="继承自所属设备" />
          <div class="point-inherit-hint">继承自设备「{{ currentDeviceName }}」，如需更换请到设备列表修改总线。</div>
        </el-form-item>
        <el-form-item label="协议" prop="protocol">
          <!-- N3: 协议同样继承自设备（设备协议又由模板决定），避免点位按错误协议解析报文。 -->
          <el-input :model-value="pointForm.protocol ? (protocolLabel[pointForm.protocol] || pointForm.protocol) : ''"
            disabled placeholder="继承自所属设备" />
          <div class="point-inherit-hint">协议继承自设备，不可单独修改。</div>
        </el-form-item>
        <el-form-item label="地址" prop="address">
          <!-- 地址格式随协议不同（Modbus 40001 / S7 DB1.DBD0 / BACnet analog-input:1），
               提示文本统一取后端协议描述符，避免前端只提示 Modbus 写法。 -->
          <el-input v-model="pointForm.address" :placeholder="protocolAddressPlaceholder(pointForm.protocol)" />
        </el-form-item>
        <el-form-item label="从站号" prop="slave_id">
          <el-input-number v-model="pointForm.slave_id" :min="1" :max="247" controls-position="right" style="width: 100%" />
        </el-form-item>
        <el-form-item label="地址偏移" prop="address_num">
          <el-input-number v-model="pointForm.address_num" :min="0" controls-position="right" style="width: 100%" />
        </el-form-item>
        <el-form-item label="采集周期" prop="interval">
          <!-- 上界跟随北向上报周期；已有大周期点位保留原值（编辑不应静默改频率） -->
          <div class="point-pair">
            <el-input-number
              v-model="pointForm.interval"
              :min="1"
              :max="Math.max(intervalMax, pointForm.interval)"
              controls-position="right"
              style="width: 100%"
            />
            <span style="color: var(--text-secondary); font-size: 0.75rem">秒</span>
          </div>
          <div class="point-inherit-hint">必须小于北向上报周期（当前 {{ northInterval }} 秒）；上报周期在「系统配置 → MQTT」中修改。</div>
        </el-form-item>
        <el-form-item label="允许写入" prop="writable">
          <el-switch v-model="pointForm.writable" />
          <span style="margin-left: 8px; color: var(--text-secondary); font-size: 0.75rem">开启后该点位可在「点位读写」页面写入，也可通过「远程控制」下发；关闭（默认）则拒绝一切写入</span>
        </el-form-item>

        <!-- BUG-B: 高级参数。这些字段后端一直支持，但表单此前不收集，
             用户填的告警/死区/聚合会被后端静默丢弃。 -->
        <el-divider content-position="left">量程与告警</el-divider>
        <el-form-item label="单位" prop="unit">
          <el-input v-model="pointForm.unit" placeholder="如: ℃ / kWh（可留空）" />
        </el-form-item>
        <el-form-item label="倍率 / 偏移">
          <div class="point-pair">
            <el-input-number v-model="pointForm.scale" :step="0.1" :precision="4" controls-position="right" style="width: 100%" />
            <el-input-number v-model="pointForm.offset" :step="0.1" :precision="4" controls-position="right" style="width: 100%" />
          </div>
          <div class="point-inherit-hint">实际值 = 原始值 × 倍率 + 偏移；倍率不能为 0（1:1 透传请填 1）。</div>
        </el-form-item>
        <el-form-item label="计算公式" prop="formula">
          <el-input v-model="pointForm.formula" placeholder="如: raw * 0.1 + 5（填写后优先于倍率/偏移）" />
        </el-form-item>
        <el-form-item label="死区 / 变化上传">
          <div class="point-pair">
            <el-input-number v-model="pointForm.deadband" :min="0" :step="0.1" :precision="4" controls-position="right" style="width: 100%" />
            <el-switch v-model="pointForm.on_change" active-text="变化才上传" />
          </div>
          <div class="point-inherit-hint">死区：变化幅度小于该值时不产生新数据帧（0 = 不启用）。</div>
        </el-form-item>
        <el-form-item label="告警高/低限">
          <div class="point-pair">
            <el-input-number v-model="pointForm.alarm_high" :precision="3" controls-position="right" placeholder="高限，留空=不启用" style="width: 100%" />
            <el-input-number v-model="pointForm.alarm_low" :precision="3" controls-position="right" placeholder="低限，留空=不启用" style="width: 100%" />
          </div>
          <div class="point-inherit-hint">超过高限或低于低限时该点位标记为告警；留空表示该侧不启用。</div>
        </el-form-item>
        <el-form-item label="聚合">
          <div class="point-pair">
            <el-select v-model="pointForm.aggregation_fn" placeholder="不聚合" clearable style="width: 100%">
              <el-option label="不聚合" value="" />
              <el-option label="最新值" value="last" />
              <el-option label="平均值" value="avg" />
              <el-option label="最大值" value="max" />
              <el-option label="最小值" value="min" />
              <el-option label="求和" value="sum" />
            </el-select>
            <el-input-number v-model="pointForm.aggregation_window" :min="1" :max="3600" :disabled="!pointForm.aggregation_fn" controls-position="right" style="width: 100%" />
          </div>
          <div class="point-inherit-hint">聚合窗口（秒）：窗口内多次采样合并成一个上报值；选「不聚合」则逐帧上报。</div>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="formGuard.close()">取消</el-button>
        <el-button type="primary" :loading="submitting" @click="submitPoint">{{ formMode === 'create' ? '创建' : '保存' }}</el-button>
      </template>
    </el-dialog>

    <PointsBatchEditDialog v-model="showBatchEdit" :selection="selection" :selected-device="selectedDevice" @edited="loadPoints" />
    <PointsGenerateDialog v-model="showGen" :device-id="selectedDevice" :device-name="currentDeviceName" :devices="devices" @generated="loadPoints" />
  </div>
</template>
<script setup lang="ts">
import { computed, h, nextTick, ref, watch } from 'vue'
import { useRouter, useRoute } from 'vue-router'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import {
  ElButton, ElDropdown, ElDropdownItem, ElDropdownMenu, ElIcon, ElSwitch, ElTag,
} from 'element-plus'
import { ArrowDown, CopyDocument, Edit, MagicStick, Reading, Refresh, Search, Upload, Plus } from '@element-plus/icons-vue'
import { usePoints } from '@/composables/devices/usePoints'
import { protocolAddressPlaceholder } from '@/composables/devices/useProtocol'
import { useTablePager } from '@/composables/core/useTablePager'
import { useQueryState } from '@/composables/core/useQueryState'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import { usePermission } from '@/composables/auth/usePermission'
import InlineEditCell from '@/components/InlineEditCell.vue'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
// 必须显式 import：这三个文件在 src/views/points/dialogs/ 下，而 unplugin-vue-components
// 默认只扫 src/components/，扫不到就只在控制台留一条 Failed to resolve component，
// 按钮点得动但弹窗永远不出现（静默失效）。
import PointsCopyDialog from './dialogs/PointsCopyDialog.vue'
import PointsBatchEditDialog from './dialogs/PointsBatchEditDialog.vue'
import PointsGenerateDialog from './dialogs/PointsGenerateDialog.vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn, protocolColumn } from '@/utils/commonColumns'
import type { PointRow } from '@/composables/devices/usePoints'

const router = useRouter()
const route = useRoute()
function goDevices() { router.push("/devices") }

const {
  devices, points, mergedPoints, loading, loadError, selectedDevice, currentDeviceName,
  northInterval, selection,
  pointForm, pointFormRef, pointRules, formMode, submitting, showForm,
  showCopy, showBatchEdit, showGen,
  protocolLabel,
  onSelectionChange, loadPoints, loadDevices,
  readPoint, removePoint, batchDelete,
  openCreate, openEdit, resetForm, submitPoint, toggleWritable, savePointField,
} = usePoints()

/**
 * 采集周期输入框上界：必须小于北向上报周期（后端 validate_cycle_conflict）。
 * 下限取 1，避免上报周期被配成 1s 时算不出合法区间。
 */
const intervalMax = computed(() => Math.max(1, northInterval.value - 1))

// 模板用字符串 ref="pointFormRef" 反向绑定该 Ref，脚本侧没有直接读取点（校验发生在
// Composable 的 submitPoint 内）。noUnusedLocals 不把字符串 ref 计入「已使用」，故显式取值。
void pointFormRef

// 按钮级权限：写操作（增删改 / 批量导入 / 批量生成）仅 point.write 可见
const { can } = usePermission()
const canWritePoint = computed(() => can('point.write'))

// 未保存更改守卫：新增/编辑点位弹窗。弹窗级拦 X/ESC/取消；scope 桥接路由级注册表，
// 弹窗开着跳路由 / 切 Hub tab 仍被 router.beforeEach / HubShell 拦截。
const formGuard = useDialogDirtyGuard(showForm, () => pointForm, { scope: route.path })

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
 * 表头排序：从站号升序（1 号排在最前）符合现场按地址排查的习惯；
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
    setTimeout(() => {
      const row = document.querySelector('.points .el-table__row.is-highlighted') as HTMLElement | null
      if (row) {
        row.scrollIntoView({ behavior: 'smooth', block: 'center' })
        row.classList.add('is-highlighted-flash')
        setTimeout(() => row.classList.remove('is-highlighted-flash'), 3000)
      }
    }, 100)
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
  { key: 'name', title: '名称' },
  { key: 'bus', title: '总线' },
  { key: 'protocol', title: '协议' },
  { key: 'address', title: '地址' },
  { key: 'slave_id', title: '从站号' },
  { key: 'writable', title: '允许写入' },
]
const { hidden: hiddenColumns } = useColumnPrefs('points', {
  // 1366 笔记本上 9 列点位表必然横向滚动：默认收起两列「每行都一样/只是溯源」的信息，
  // 「从站号」「操作」等排查与操作必需的列始终可见（用户可在显示设置里勾回来）。
  narrowHidden: ['_source', 'bus'],
})

/** 模板展开点位的只读文本单元格（无配置元数据时显示占位符） */
function readonlyCell(v: string | number | null | undefined) {
  return h('span', { class: 'cell-readonly' }, v ?? '—')
}

const pointColumns: ColumnDef<PointRow>[] = [
  { key: 'id', title: '点位 ID', minWidth: 160, ellipsis: true, mono: true, sortable: true },
  {
    key: '_source',
    title: '来源',
    width: 80,
    // 模板展开点位不落 config.points，仅随运行时存在：标清来源，避免误以为可编辑
    cell: (row) =>
      row._source === 'template'
        ? h(ElTag, { type: 'info', size: 'small', effect: 'plain', round: true }, () => '模板')
        : h(ElTag, { type: 'success', size: 'small', effect: 'plain', round: true }, () => '配置'),
  },
  {
    key: 'name',
    title: '名称',
    width: 140,
    sortable: true,
    // 名称/地址/从站号支持双击行内改：改个错别字不该再走一遍弹窗；模板行只读
    cell: (row) =>
      row._source === 'template'
        ? readonlyCell(row.name)
        : h(InlineEditCell, {
            value: row.name,
            label: '名称',
            maxlength: 64,
            save: (v: string | number) => savePointField(row, { name: String(v) }),
          }),
  },
  { key: 'bus', title: '总线', width: 90, sortable: true },
  protocolColumn<PointRow>({ width: 96, sortable: true }),
  {
    key: 'address',
    title: '地址',
    width: 96,
    sortable: true,
    cell: (row) =>
      row._source === 'template'
        ? readonlyCell(row.address || null)
        : h(InlineEditCell, {
            value: row.address,
            label: '地址',
            mono: true,
            align: 'right',
            maxlength: 64,
            save: (v: string | number) => savePointField(row, { address: String(v) }),
          }),
  },
  {
    key: 'slave_id',
    title: '从站号',
    width: 76,
    sortable: true,
    cell: (row) =>
      row._source === 'template'
        ? readonlyCell(row.slave_id)
        : h(InlineEditCell, {
            value: row.slave_id,
            label: '从站号',
            type: 'number',
            min: 1,
            max: 247,
            mono: true,
            align: 'right',
            save: (v: string | number) => savePointField(row, { slave_id: Number(v) }),
          }),
  },
  {
    key: 'writable',
    title: '允许写入',
    width: 84,
    // S11: 白名单显式开关——before-change 返回 Promise，仅后端成功后才翻转
    // 模板行开关禁用：白名单归属配置，要改请在设备模板中调整
    cell: (row) =>
      h(ElSwitch, {
        modelValue: row.writable,
        size: 'small',
        disabled: row._source === 'template',
        beforeChange: () => toggleWritable(row),
      }),
  },
  actionColumn<PointRow>({
    // 固定在右缘（fixed: 'right'）：横向滚动时操作入口始终在手边，不会被滚出可视区。
    // 宽度收窄到 126px：编辑 + 「更多」两个入口足够，避免列宽挤压地址/从站号。
    width: 126,
    fixed: 'right',
    // 高频操作（编辑）直接露出，读取/删除收进「更多」：
    // 3 个同权重文字按钮会把操作列撑到 260px，挤压地址、从站号等信息列。
    // 按钮级权限：编辑/删除仅 point.write 可见；读取当前值所有角色可见。
    cell: (row) => {
      // 模板展开点位只读：只允许读取当前值，编辑/删除需改设备模板
      if (row._source === 'template') {
        return h('div', { class: 'row-actions' }, [
          h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => readPoint(row) },
            () => [h(ElIcon, null, () => h(Reading)), ' 读取当前值']),
        ])
      }
      const children = []
      if (canWritePoint.value) {
        children.push(
          h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => openEdit(row) },
            () => [h(ElIcon, null, () => h(Edit)), ' 编辑']),
        )
      }
      // 「更多」下拉：至少有读取动作就显示；写权限时追加删除项
      const dropdownItems = [
        h(ElDropdownItem, { command: 'read' },
          () => [h(ElIcon, null, () => h(Reading)), ' 读取当前值']),
      ]
      if (canWritePoint.value) {
        dropdownItems.push(h(ElDropdownItem, { command: 'delete', divided: true }, () => '删除点位'))
      }
      children.push(
        h(ElDropdown, {
          size: 'small',
          trigger: 'click',
          onCommand: (cmd: string) => (cmd === 'read' ? readPoint(row) : removePoint(row)),
        }, {
          default: () => h(ElButton, { size: 'small', type: 'primary', link: true },
            () => ['更多', h(ElIcon, { class: 'el-icon--right' }, () => h(ArrowDown))]),
          dropdown: () => h(ElDropdownMenu, null, () => dropdownItems),
        }),
      )
      return h('div', { class: 'row-actions' }, children)
    },
  }),
]
</script>

<style scoped src="./PointList.css"></style>