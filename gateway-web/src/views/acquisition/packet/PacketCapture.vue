<template>
  <div class="cap">
    <SectionHeader title="报文捕获" icon="Monitor" />

    <FilterToolbar v-model="filters" :fields="filterFields" class="toolbar">
      <el-button type="primary" size="small" @click="toggleCapture" :icon="capturing ? VideoPause : VideoPlay">{{ capturing ? '停止' : '开始抓包' }}</el-button>
      <el-button size="small" @click="clear">清空</el-button>
      <el-button size="small" :disabled="!filtered.length" @click="copyFiltered">复制报文</el-button>
      <el-button size="small" :disabled="!filtered.length" @click="exportFiltered">导出 CSV</el-button>
      <el-button size="small" :disabled="!hasRequest" @click="saveAsSession">存为回放会话</el-button>
      <span class="count" aria-live="polite">共 {{ packets.length }} 包{{ filtered.length !== packets.length ? `（筛选后 ${filtered.length} 包）` : '' }}</span>
    </FilterToolbar>

    <DataTable
      :data="filtered"
      :columns="columns"
      row-key="id"
      :row-class-name="rowClass"
      :max-height="600"
      empty-text="暂无报文"
    />

    <el-drawer v-model="detailVisible" title="报文详情" size="42%">
      <div v-if="current" class="detail">
        <el-descriptions :column="1" border>
          <el-descriptions-item label="方向">{{ current.direction === 'request' ? '请求' : '响应' }}</el-descriptions-item>
          <el-descriptions-item label="设备">{{ current.deviceName }}</el-descriptions-item>
          <el-descriptions-item label="总线">{{ current.bus }}</el-descriptions-item>
          <el-descriptions-item label="时间">{{ formatDateTime(current.time) }}</el-descriptions-item>
          <el-descriptions-item label="延迟">{{ current.delayMs ? `${current.delayMs}ms` : '-' }}</el-descriptions-item>
          <el-descriptions-item label="解析">{{ current.parsed || '-' }}</el-descriptions-item>
        </el-descriptions>
        <h4>原始报文
          <el-button size="small" link @click="copyCurrent">复制</el-button>
          <el-button size="small" link type="primary" @click="sendToEditor">发送到报文编辑</el-button>
        </h4>
        <!-- 审查#7：HEX + ASCII 双栏对照，取代单栏 hex 文本 -->
        <HexAsciiView :hex="current.hex" />
      </div>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { h, ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { usePolling } from '@/composables/core/usePolling'
import { useRouter } from 'vue-router'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { useQueryState } from '@/composables/core/useQueryState'
const { showToast } = useErrorDisplay()
import { ElButton, ElTag, ElMessage } from 'element-plus'
import {VideoPlay, VideoPause} from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import {startCapture, stopCapture, getPackets, clearPackets, type Packet} from '@/api/protocolTools'
import FilterToolbar, { type FilterField } from '@/components/FilterToolbar.vue'
import { useDebugTargets } from '@/composables/acquisition/useDebugTargets'
import { copyText } from '@/utils/clipboard'
import { downloadTableCsv } from '@/utils/exportCsv'
import { formatDateTime, formatTimeOfDay } from '@/utils/datetime'
import HexAsciiView from '@/components/industrial/HexAsciiView.vue'
import { saveSession } from '@/api/protocolTools'
import { buildReplaySession, defaultSessionName } from '@/utils/replaySession'

const packets = ref<Packet[]>([])
const capturing = ref(false)
// 筛选条件与 URL 双向同步：切 tab 返回后还原，且可刷新/分享带筛选的链接
const qsBus = useQueryState('bus')
const qsDevice = useQueryState('device')
const filters = ref<{ bus: string; device: string }>({ bus: qsBus.value, device: qsDevice.value })
watch(filters, (f) => {
  qsBus.value = f.bus
  qsDevice.value = f.device
}, { deep: true })
// 总线筛选选项来自真实配置（port_name），与报文体 bus 字段一致；不再用静态 BUSES 常量
const { busOptions } = useDebugTargets()
const filterFields = computed<FilterField[]>(() => [
  { key: 'bus', type: 'select', placeholder: '总线', width: 160, options: busOptions.value },
  { key: 'device', type: 'input', placeholder: '设备ID', width: 140 },
])

const filtered = computed(() => packets.value.filter((p) => (!filters.value.bus || p.bus === filters.value.bus) && (!filters.value.device || p.deviceId.includes(filters.value.device))))

// 没有请求帧的会话无法回放（回放只下发请求帧），此时禁用保存入口
const hasRequest = computed(() => filtered.value.some((p) => p.direction === 'request'))

// 方向底色：请求蓝 / 响应绿，取代原 .packet.request / .packet.response
function rowClass(row: Packet) {
  return row.direction === 'request' ? 'row-request' : 'row-response'
}

// 详情抽屉：承载被列省略号截断的完整报文
const detailVisible = ref(false)
const current = ref<Packet | null>(null)
function showDetail(row: Packet) {
  current.value = row
  detailVisible.value = true
}

/**
 * 单条报文整理成纯文本：现场排查需要把报文原文贴给研发，
 * 截图既不可检索也无法复制 hex，这里统一成"时间 + 方向 + 设备 + 帧"的格式。
 */
function packetToText(p: Packet): string {
  const dir = p.direction === 'request' ? '请求' : '响应'
  const time = formatDateTime(p.time)
  const lines = [`[${time}] ${dir} 设备=${p.deviceName} 总线=${p.bus}`]
  if (p.delayMs) lines.push(`延迟: ${p.delayMs}ms`)
  if (p.parsed) lines.push(`解析: ${p.parsed}`)
  lines.push(p.hex)
  return lines.join('\n')
}

function copyCurrent() { if (current.value) void copyText(packetToText(current.value), '报文详情已复制') }
/** 复制当前筛选结果（而非全部）：先筛出故障设备再复制，贴出去的就是要排查的那一段 */
function copyFiltered() { void copyText(filtered.value.map(packetToText).join('\n\n'), `已复制 ${filtered.value.length} 条报文`) }

/** 导出当前筛选结果为 CSV：现场离线环境无法复制粘贴时的交付证据 */
function exportFiltered() {
  downloadTableCsv(
    ['时间', '方向', '设备', '总线', '延迟ms', '报文HEX', '解析'],
    filtered.value.map((p) => [
      formatDateTime(p.time), p.direction === 'request' ? '请求' : '响应',
      p.deviceName, p.bus, p.delayMs ?? '', p.hex, p.parsed,
    ]),
    `packet-capture-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.csv`,
  )
}

/**
 * 抓包 → 报文回放闭环：把当前筛选出的真帧（含响应帧，供回放比对）存为回放会话，
 * 保存后到「报文回放」tab 选中即可对设备复现这一段交互。
 */
async function saveAsSession() {
  const frames = filtered.value.map((p) => ({ time: p.time, direction: p.direction, hex: p.hex, parsed: p.parsed }))
  const name = prompt('回放会话名称：', defaultSessionName())
  if (!name) return
  // 设备名仅作展示标识；混抓多设备时如实标注
  const devNames = new Set(filtered.value.map((p) => p.deviceName))
  const deviceName = devNames.size === 1 ? [...devNames][0]! : '多设备混抓'
  try {
    await saveSession(buildReplaySession(name, deviceName, frames))
    ElMessage.success(`已保存回放会话「${name}」（${frames.length} 帧），请到报文回放页执行`)
  } catch (e) {
    showToast(e, '保存回放会话失败')
  }
}

/** 抓包 → 报文编辑闭环：携带 hex 跳转到编辑器 tab，免去手抄报文 */
const router = useRouter()
function sendToEditor() {
  if (!current.value) return
  detailVisible.value = false
  router.push({ path: '/acquisition/console', query: { tab: 'packet-editor', hex: current.value.hex } })
}

// 表格统一走 DataTable；超过阈值自动切虚拟滚动，取代原「仅渲染最近 500 包」的硬截断
const columns: ColumnDef<Packet>[] = [
  {
    key: 'direction',
    title: '方向',
    width: 70,
    cell: (row) => h(ElTag, { size: 'small', type: row.direction === 'request' ? 'primary' : 'success' }, () => (row.direction === 'request' ? '请求' : '响应')),
  },
  { key: 'time', title: '时间', width: 100, cell: (row) => formatTimeOfDay(row.time) },
  { key: 'deviceName', title: '设备', minWidth: 120 },
  { key: 'bus', title: '总线', width: 80 },
  { key: 'delayMs', title: '延迟', width: 80, cell: (row) => (row.delayMs ? `${row.delayMs}ms` : '-') },
  { key: 'hex', title: '报文', minWidth: 260, ellipsis: true, cell: (row) => h('code', { class: 'hex' }, row.hex) },
  { key: 'parsed', title: '解析', minWidth: 200, ellipsis: true, cell: (row) => row.parsed },
  actionColumn<Packet>({ width: 70, fixed: 'right', cell: (row) => h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => showDetail(row) }, () => '详情') }),
]

async function load() { packets.value = await getPackets(100) }

// 抓包轮询：usePolling 统一提供隐藏暂停 / 失败退避 / 卸载清理（后端持续抓包，前端暂停拉取无数据损失）
const { start: startPoll, stop: stopPoll } = usePolling(load, 500, { autoStart: false })

function toggleCapture() {
  if (capturing.value) {
    stopCapture(); capturing.value = false
    stopPoll()
  } else {
    startCapture(filters.value.device || undefined); capturing.value = true
    startPoll()
  }
}
async function clear() { try { await clearPackets(); load() } catch (e) { showToast(e, '清空失败') } }

onMounted(load)

// 卸载清理：若正在抓包则调用 stopCapture；轮询定时器由 usePolling 在卸载时自动清理。
onUnmounted(() => {
  if (capturing.value) {
    stopCapture()
    capturing.value = false
  }
})
</script>

<style scoped>
.cap { padding: 0px; }
.toolbar { display: flex; gap: 8px; align-items: center; margin-bottom: 12px; }
.count { margin-left: auto; color: var(--el-text-color-secondary); font-size: 0.8125rem; }
.hex { color: var(--el-color-primary); font-size: 0.8125rem; letter-spacing: 1px; font-family: monospace; }
/* el-table 行底色：方向区分（class 由 rowClassName 注入到 <tr>） */
:deep(.row-request) td { background: rgba(var(--color-primary-rgb), 0.05); }
:deep(.row-response) td { background: rgba(var(--color-success-rgb), 0.05); }
</style>