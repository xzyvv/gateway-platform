<template>
  <div class="devices">
    <el-card shadow="never" class="section-card">
      <template #header>
        <!-- 页面标题由 HubShell 按页签渲染（设备清单），这里只保留数量徽标与操作区，
             避免同一屏出现「设备管理 / 设备清单 / 设备管理」三层名字 -->
        <div class="list-toolbar">
          <el-tag type="info" effect="plain" round size="small" class="count-badge">
            {{ keyword.trim() ? `${total} / ${deviceRows.length} 台` : `${deviceRows.length} 台` }}
          </el-tag>
          <div class="list-actions">
            <el-input
              v-model="keyword"
              class="search-input"
              placeholder="搜索 ID / 名称 / 模板 / 分组"
              aria-label="搜索设备"
              clearable
            >
              <template #prefix><el-icon><Search /></el-icon></template>
            </el-input>
            <el-button size="small" @click="loadDevices"><el-icon><Refresh /></el-icon> 刷新</el-button>
            <el-button type="warning" size="small" @click="showDiscover = true"><el-icon><Search /></el-icon> 扫描发现</el-button>
            <el-button type="success" size="small" :loading="checking" @click="batchCheck"><el-icon><Connection /></el-icon> 一键连通性检测</el-button>
            <el-button v-if="canWriteDevice" type="primary" size="small" @click="addDevice"><el-icon><Plus /></el-icon> 新增设备</el-button>
            <!-- 表格显示设置：行密度全站共享，列显隐按设备表记忆 -->
            <TablePrefsMenu table-id="devices" :columns="toggleableColumns" />
          </div>
        </div>
      </template>

      <div class="device-table">
        <!-- 加载失败显式提示：静默空表会让人以为设备被删了 -->
        <el-alert
          v-if="loadError"
          type="error"
          :closable="false"
          show-icon
          class="load-error"
          :title="`设备列表加载失败：${loadError}`"
        >
          <el-button size="small" type="danger" plain @click="loadDevices">重试</el-button>
        </el-alert>
        <!-- 分页与空态已下沉到 DataTable，页面只提供数据与动作 -->
        <DataTable
          :data="pagedRows"
          :columns="deviceColumns"
          :hidden-columns="hiddenColumns"
          row-key="id"
          stripe
          :row-class-name="deviceRowClass"
          :loading="loading"
          :sort-key="sortKey"
          :sort-order="sortOrder"
          :empty-text="emptyDescription"
          :empty-action-text="keyword.trim() ? '清空搜索' : '立即新增'"
          v-model:page="page"
          v-model:page-size="pageSize"
          :total="total"
          :page-sizes="pageSizes"
          @sort-change="onSortChange"
          @empty-action="keyword.trim() ? (keyword = '') : addDevice()"
        />
      </div>
    </el-card>

    <DeviceDiscoverDialog v-model="showDiscover" :protocol-options="protocolOptions" :devices="devices" @add="onDiscoverAdd" />

    <el-dialog v-model="showCreate" :title="editing ? '编辑设备' : '新增设备'" width="520px" :before-close="formGuard.beforeClose" @open="onDialogOpen">
      <el-form ref="formRef" :model="form" :rules="rules" label-width="90px" class="device-form">
        <el-form-item label="设备名称" prop="name">
          <el-input v-model="form.name" placeholder="请输入设备名称">
            <template #prefix><el-icon><Cpu /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-form-item label="模板" prop="template">
          <!-- S3: 改为下拉选择。原自由文本框 placeholder 给的是文件名，后端却按模板 name 查找，
               照填必失败；改为直接从后端模板库选，顺带展示协议与点位数。 -->
          <el-select
            v-model="form.template"
            filterable
            allow-create
            default-first-option
            :loading="templatesLoading"
            placeholder="选择设备模板（决定采集哪些点位）"
            style="width: 100%"
            @change="applyTemplateProtocol"
          >
            <el-option v-for="t in templates" :key="t.name" :label="t.name" :value="t.name">
              <span>{{ t.name }}</span>
              <span class="tpl-opt-meta">{{ t.protocol || '—' }} · {{ (t.points || []).length }} 点位</span>
            </el-option>
          </el-select>
          <div v-if="templateLoadError" class="tpl-hint tpl-hint--err">{{ templateLoadError }}</div>
          <div v-else class="tpl-hint">模板决定该设备采集的点位清单；可选模板来自网关的模板库。</div>
        </el-form-item>
        <el-form-item label="总线" prop="bus">
          <!-- N1: 改为下拉选择。原自由文本框填错总线名仍能保存成功，
               后端按 bus 建不出采集器，设备永远采不到数据。 -->
          <BusSelect
            v-model="form.bus"
            :options="busSelectOptions"
            filterable
            :loading="busesLoading"
            :width="'100%'"
            placeholder="选择设备挂载的总线（来自运行参数）"
          />
          <div v-if="busLoadError" class="tpl-hint tpl-hint--err">
            {{ busLoadError }}
            <router-link class="tpl-hint__link" to="/system/config?tab=runtime-config">去新增总线</router-link>
          </div>
          <div v-else class="tpl-hint">总线决定设备的物理连接；可选总线来自「系统设置 → 运行参数」。</div>
        </el-form-item>
        <el-form-item label="协议" prop="protocol">
          <!-- N2: 协议由模板决定。原可单独选择，与模板不一致时会按错误协议读寄存器，
               整台设备全点位失败；现置只读，随模板自动带出。 -->
          <el-input
            :model-value="form.protocol ? (protocolLabel[form.protocol] || form.protocol) : ''"
            disabled
            placeholder="选择模板后自动带出"
          />
          <div class="tpl-hint">协议由模板决定，不可单独修改。</div>
        </el-form-item>
        <el-form-item label="设备地址" prop="address">
          <el-input v-model="form.address" :placeholder="protocolAddressPlaceholder(form.protocol)">
            <template #prefix><el-icon><Location /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-form-item v-if="form.protocol === 'modbus'" label="从站地址" prop="slave_id">
          <el-input-number v-model="form.slave_id" :min="1" :max="247" controls-position="right" placeholder="1-247（可选）" style="width: 100%" />
        </el-form-item>
        <el-form-item label="采集周期" prop="interval">
          <el-input-number v-model="form.interval" :min="1" controls-position="right" placeholder="留空使用模板默认" style="width: 100%" />
          <span style="margin-left: 8px; color: var(--text-secondary); font-size: 0.75rem">单位：秒；留空则使用模板默认周期</span>
        </el-form-item>
        <el-form-item label="分组">
          <el-input v-model="form.group" placeholder="如: 1号楼/机房A（可选）" clearable>
            <template #prefix><el-icon><CollectionTag /></el-icon></template>
          </el-input>
        </el-form-item>
        <el-form-item label="标签">
          <el-select v-model="form.tags" multiple filterable allow-create default-first-option placeholder="输入标签后回车新增（可选）" style="width: 100%" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="formGuard.close()"><el-icon><Close /></el-icon> 取消</el-button>
        <el-button :loading="testingConn" @click="testDeviceConn"><el-icon><Connection /></el-icon> 测试连接</el-button>
        <el-button type="primary" :loading="saving" @click="saveDevice"><el-icon><Check /></el-icon> 保存</el-button>
      </template>
    </el-dialog>

    <!-- A7：资产台账并入设备详情。原「资产台账」是独立页签，用户要先在台账表里找设备、
         再回设备清单核对，两个方向都要跳；改为从设备行直接打开该设备的资产表单 -->
    <DeviceAssetDialog
      v-model="assetShow"
      :device-id="assetDeviceId"
      :can-write="canWriteDevice"
    />
  </div>
</template>

<script setup lang="ts">
import { computed, ref, watch, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { useDevices } from '@/composables/devices/useDevices'
import { useTablePager } from '@/composables/core/useTablePager'
import { useQueryState } from '@/composables/core/useQueryState'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import { usePermission } from '@/composables/auth/usePermission'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
import BusSelect, { type BusOption } from '@/components/industrial/BusSelect.vue'
import DeviceAssetDialog from './dialogs/DeviceAssetDialog.vue'
import DeviceDiscoverDialog from './dialogs/DeviceDiscoverDialog.vue'
import {
  usePointBackoff,
  lastGoodByDevice,
  resolveBackoffDeviceId,
} from '@/composables/data/usePointBackoff'
import type { PointBackoff } from '@/api/dataQuality'
import { getPackets } from '@/api/protocolTools/packetCapture'
import type { Packet } from '@/api/protocolTools/types'
import {
  aggregateBackoffByDevice,
  aggregateLastFrameByDevice,
  mergeDeviceEvidence,
  type DeviceEvidence,
} from '@/utils/deviceEvidence'
import {
  createDeviceColumns,
  type BackoffQuality,
  type DeviceOnlineState,
  type DeviceRow,
} from './deviceListColumns'

const {
  devices, loading, loadError, saving, testingConn, showCreate, showDiscover, editing, checking,
  form, formRef, rules, formGuard,
  templates, templatesLoading, templateLoadError,
  buses, busesLoading, busLoadError, applyTemplateProtocol,
  commsStats, commsQualityType, commsQualityText,
  protocolLabel, protocolOptions, protocolAddressPlaceholder,
  rowClass,
  loadDevices, batchCheck, addDevice, addDeviceWithTemplate, editDevice, onDialogOpen,
  testDeviceConn, saveDevice, removeDevice, onDiscoverAdd,
  saveDeviceField,
} = useDevices()

// 模板里的 ref="formRef" 是按名字绑定 setup 变量的，vue-tsc 的 noUnusedLocals 看不到这层引用
void formRef

/** 总线选项：label 用 port_name（写进 form.bus 的就是它），协议名放右侧 meta */
const busSelectOptions = computed<BusOption[]>(() =>
  buses.value.map((b) => ({
    label: b.port_name,
    value: b.port_name,
    meta: protocolLabel[b.protocol] || b.protocol,
  })),
)

// 按钮级权限：写操作（新增/编辑/删除）仅 device.write 可见
const { can } = usePermission()
const canWriteDevice = computed(() => can('device.write'))

// ===== 资产信息（A7：由「资产台账」页签并入设备行）=====
// 表单本体在 DeviceAssetDialog.vue；本页只保留「哪个设备的资产」这一个状态。
const assetShow = ref(false)
const assetDeviceId = ref('')

/** 打开某设备的资产表单：已登记则回填，未登记（接口返回空/失败）按新增处理 */
function openAsset(row: DeviceRow) {
  assetDeviceId.value = row.id
  assetShow.value = true
}

// 表格统一走 DataTable（A4）：行数据带扩展字段，故按 DeviceRow 视图使用
// 注入 last_good_ts 供「最后成功采集」列展示与排序；无数据时为 undefined（排序沉底）
const deviceRows = computed<DeviceRow[]>(() =>
  devices.value.map((d) => ({
    ...(d as DeviceRow),
    last_good_ts: lastGoodByDeviceMap.value[d.id] || undefined,
  })),
)
const deviceRowClass = (row: DeviceRow) => rowClass({ row })

// ===== 最后成功采集时间（区分「设备坏了」与「值一直不变」）=====
// 数据来自 /api/devices/backoff 的 PointBackoff.last_good_ts（毫秒，0=从未成功）。
// 聚合口径（含空 device_id 的前缀反解）统一在 usePointBackoff，避免本页另写一套。
const { points: backoffList, load: loadBackoff } = usePointBackoff()
onMounted(loadBackoff)
// 设备列表刷新时同步刷新退避数据（新增/删除设备后 last_good_ts 映射会变）
watch(devices, loadBackoff)

const lastGoodByDeviceMap = computed<Record<string, number>>(() =>
  lastGoodByDevice(
    backoffList.value,
    devices.value.map((d) => d.id),
  ),
)

// ===== 设备维度证据（诊断闭环，P2 #5）=====
// 目标：让「某设备为什么离线」在一屏可答——除了结论（最后成功时间 / 通信质量），
// 还要给出支撑判断的原始证据：最近一帧报文的 hex 与失败原因。
// 数据源都是后端已有接口：/api/devices/backoff（统计）+ 抓包缓冲（原始帧）。
const packets = ref<Packet[]>([])
async function loadPackets() {
  try {
    packets.value = await getPackets(200)
  } catch { /* 抓包接口不可用：证据列显示「—」，不影响主流程 */ }
}
onMounted(loadPackets)

// S7: 承接「设备模板」页「用此模板新建设备」的跳转（/devices?action=create&template=xxx）。
// 打开新增弹窗并把模板预选好，让按钮真正把用户带到下一步，而不是只弹一条提示。
const route = useRoute()
const router = useRouter()
onMounted(() => {
  if (route.query.action !== 'create') return
  // 模板预填交给 addDeviceWithTemplate：弹窗 open 事件会 resetForm，
  // 在这里直接赋 form.template 会被随后的 reset 清掉（实测弹窗里仍是空）。
  const tpl = typeof route.query.template === 'string' ? route.query.template : ''
  if (tpl) addDeviceWithTemplate(tpl)
  else addDevice()
})

// 设备列表刷新时同步刷新抓包缓冲：新增/删除设备后归属关系会变
watch(devices, loadPackets)

/** 设备 → 证据（最后成功时间 + 连续失败数 + 最近原始帧 hex） */
const evidenceByDevice = computed<Record<string, DeviceEvidence>>(() =>
  mergeDeviceEvidence(
    aggregateBackoffByDevice(backoffList.value),
    aggregateLastFrameByDevice(packets.value),
  ),
)

/**
 * 通信质量兜底（后端真实采集质量）：commsStats 是前端本会话的 UI 读写聚合，
 * 刷新即空 → 设备列表会永远显示「暂无数据」。改用 /api/devices/backoff 的
 * 点位质量按设备聚合：全 good → 优；含 bad（连续失败≥3）→ 差；否则 → 波动。
 */
function backoffDeviceId(b: PointBackoff): string {
  // 归属反解统一走 usePointBackoff，避免与本页 lastGoodByDevice 两套规则
  return resolveBackoffDeviceId(b, devices.value.map((d) => d.id))
}
const backoffQuality = computed<Record<string, BackoffQuality>>(() => {
  const byDev: Record<string, PointBackoff[]> = {}
  for (const b of backoffList.value) {
    const devId = backoffDeviceId(b)
    if (!devId) continue
    ;(byDev[devId] ??= []).push(b)
  }
  const map: Record<string, BackoffQuality> = {}
  for (const [devId, list] of Object.entries(byDev)) {
    const bad = list.some((b) => b.quality === 'bad')
    const warn = list.some((b) => b.quality === 'uncertain' || b.quality === 'old')
    const baseMs = list[0]?.base_interval_ms ?? 0
    map[devId] = {
      level: bad ? 'bad' : warn ? 'warn' : 'good',
      text: bad ? '差' : warn ? '波动' : '优',
      intervalLabel: baseMs > 0 ? `${(baseMs / 1000).toFixed(baseMs % 1000 ? 1 : 0)}s/次` : '',
    }
  }
  return map
})

/**
 * 设备在线态：由后端真实采集数据推导，而非设备配置里的开关字段。
 * 曾经的实现读 `row.enabled`，但 /api/devices 根本不返回该字段 → 恒为「在线」，
 * 与健康评分 critical、点位质量 stale 直接矛盾。判定口径：
 *   - 点位质量「差」（连续失败≥3）或证据里连续失败≥3 → 离线
 *   - 有成功采集时间 → 在线
 *   - 有退避/抓包记录但从未成功 → 离线
 *   - 完全没有记录（如设备无点位）→ 未知
 */
const onlineStateByDevice = computed<Record<string, DeviceOnlineState>>(() => {
  const map: Record<string, DeviceOnlineState> = {}
  for (const d of devices.value) {
    const bq = backoffQuality.value[d.id]
    const ev = evidenceByDevice.value[d.id]
    if (bq?.level === 'bad' || (ev?.consecutiveFailures ?? 0) >= 3) map[d.id] = 'offline'
    else if (ev?.lastGoodTs) map[d.id] = 'online'
    else if (bq || ev) map[d.id] = 'offline'
    else map[d.id] = 'unknown'
  }
  return map
})

/**
 * 设备列表的搜索 + 分页。
 * 现场常按分组（如「机房A」）或模板名找设备，逐行扫表效率太低。
 */
const keyword = useQueryState('kw')
const kw = computed(() => keyword.value.trim().toLowerCase())
const { page, pageSize, pageSizes, total, paged: pagedRows, reset, sortKey, sortOrder, toggleSort } = useTablePager(deviceRows, (row) => {
  if (!kw.value) return true
  return [row.id, row.name, row.template, row.bus, row.address, row.group, (row.tags ?? []).join(' ')]
    .some((v) => String(v ?? '').toLowerCase().includes(kw.value))
}, { syncUrl: true })

/** 表头排序：排序在分页入口完成，DataTable 只抛事件（它只拿到当页数据） */
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

const emptyDescription = computed(() =>
  loadError.value
    ? '设备列表加载失败，请点击上方「重试」'
    : kw.value ? `没有匹配「${keyword.value.trim()}」的设备` : '暂无设备',
)

watch(keyword, reset)
// 刷新后回到第一页：避免刷新完停在空页，让人误以为设备全丢了
watch(devices, reset)

/** 可隐藏的列：设备表 9 列，窄屏下模板/分组/标签通常用不上 */
const toggleableColumns = [
  { key: 'id', title: '设备 ID' },
  { key: 'name', title: '设备名称' },
  { key: 'last_good_ts', title: '最后成功采集' },
  { key: 'evidence', title: '诊断证据' },
  { key: 'template', title: '模板' },
  { key: 'bus', title: '总线' },
  { key: 'protocol', title: '协议' },
  { key: 'address', title: '设备地址' },
  { key: 'group', title: '分组' },
  { key: 'tags', title: '标签' },
]
const { hidden: hiddenColumns } = useColumnPrefs('devices', {
  // 窄屏（<1440px 笔记本）放下全部列需横向滚动，「设备地址」被挤出可视区（1366 下需横滚 150px）。
  // 因此窄屏默认收起六列：诊断证据 / 分组 / 标签 / 协议 / 模板 / 总线——
  // 诊断证据、分组、标签属低频维护信息；协议与模板可从设备详情、点位页查看；
  // 单总线直连调试时总线名无区分度。
  // 保留「状态 / 通信质量 / 最后成功采集 / 设备 ID / 设备名称 / 设备地址 / 操作」这组现场诊断必需列，
  // 使地址列无需横向滚动即可见。用户手动调整过的列偏好优先，不会被反复隐藏。
  narrowHidden: ['evidence', 'group', 'tags', 'protocol', 'template', 'bus'],
})

// 列定义用 getter 传入响应式来源：cell 渲染时才读取，
// 直接传 .value 快照会让列在数据刷新后仍显示旧值。
const deviceColumns = createDeviceColumns({
  get commsStats() { return commsStats.value },
  commsQualityType: (id) => commsQualityType(id),
  commsQualityText: (id) => commsQualityText(id),
  get backoffQuality() { return backoffQuality.value },
  get evidenceByDevice() { return evidenceByDevice.value },
  get onlineStateByDevice() { return onlineStateByDevice.value },
  saveDeviceField: (row, patch) => saveDeviceField(row, patch),
  openAsset,
  editDevice: (row) => editDevice(row),
  removeDevice: (row) => removeDevice(row),
  gotoPoints: (row) => router.push({ path: '/points', query: { tab: 'points', device: row.id } }),
  get canWriteDevice() { return canWriteDevice.value },
})
</script>

<style scoped src="./DeviceList.css"></style>
