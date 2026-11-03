<template>
  <div class="tester-page">
    <!-- 顶部说明 -->
    <TesterIntro :supports-write="supportsWrite" @scroll-to-batch-write="scrollToBatchWrite" />

    <div class="tester-grid">
      <!-- 左侧：选择 + 操作 -->
      <el-card class="control-card" shadow="never">
        <SectionHeader title="选择点位" icon="Cpu" />

        <DevicePointSelector
          v-model:selected-device="selectedDevice"
          v-model:selected-point="selectedPoint"
          :devices="devices"
          :points="points"
          :devices-loading="devicesLoading"
          :points-loading="pointsLoading"
          :selected-protocol="selectedProtocol"
          :caps-known="capsKnown"
          :supports-write="supportsWrite"
          :supports-batch-read="supportsBatchRead"
          :supports-scan-slaves="supportsScanSlaves"
          :supports-diag="supportsDiag"
          @point-change="onPointChange"
        />

        <CurrentValuePanel
          :has-point="!!selectedPoint"
          :current-value="currentValue"
          :value-loading="valueLoading"
          :value-error="valueError"
          :value-unit="selectedPointType"
          :diag-loading="diagLoading"
          :supports-diag="supportsDiag"
          @read="doRead"
          @diag="doDiag"
        />

        <WriteSection
          v-model:write-value="writeValue"
          :has-point="!!selectedPoint"
          :supports-write="supportsWrite"
          :caps-known="capsKnown"
          :selected-protocol="selectedProtocol"
          :write-loading="writeLoading"
          @write="doWrite"
        />
      </el-card>

      <!-- 右侧：操作记录 -->
      <OperationLogPanel :logs="logs" @export="exportLogs" @clear="clearLog" />
    </div>

    <!-- 批量写入：仅当协议声明 supports_write 时显示 -->
    <BatchWritePanel
      v-if="supportsWrite"
      :points="points"
      :device-id="selectedDevice"
      @log="addLog"
    />
  </div>
</template>

<script setup lang="ts">
/**
 * 点位读写测试页（协议仿真 / 点位读写调试）
 *
 * 页面职责：选设备与点位 → 读取当前值 / 写入测试值 / 协议诊断 → 批量写入多点位，
 * 全过程写入右侧「操作记录」并附原始报文 HEX。
 *
 * 本文件只保留「状态 + 请求编排」，展示部分按区块拆到同目录 parts/ 下：
 * - parts/TesterIntro.vue        顶部说明条
 * - parts/DevicePointSelector.vue 设备/点位选择 + 协议能力徽标
 * - parts/CurrentValuePanel.vue  当前值展示 + 读取/协议诊断按钮
 * - parts/WriteSection.vue       写入区（按 supports_write 显隐）
 * - parts/OperationLogPanel.vue  右侧操作记录（含原始报文 HEX）
 * - parts/BatchWritePanel.vue    批量写入（多点位）
 */
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import { ElMessage } from 'element-plus'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import { confirmPointWrite, usePointWrite } from '@/composables/data/usePointWrite'
import { listProtocolCapabilities, getProtocolDiag } from '@/api/protocols'
import type { ProtocolCapabilities } from '@/api/generated/ProtocolCapabilities'
import type { DeviceConfig, PointConfig } from '@/api/types'
import { recordRead as recordDeviceRead } from '@/utils/deviceComms'
import { recordBusOp } from '@/utils/busStats'
import { useSharedDebugTarget } from '@/composables/acquisition/useDebugTargets'
import { extractErrorMessage } from '@/utils/errorHandler'
import { withDiagHint } from '@/utils/diagHints'
import { formatClock } from '@/utils/datetime'
import { downloadTableCsv } from '@/utils/exportCsv'
import { getPackets } from '@/api/protocolTools/packetCapture'
import TesterIntro from './parts/TesterIntro.vue'
import DevicePointSelector from './parts/DevicePointSelector.vue'
import CurrentValuePanel from './parts/CurrentValuePanel.vue'
import WriteSection from './parts/WriteSection.vue'
import OperationLogPanel from './parts/OperationLogPanel.vue'
import BatchWritePanel from './parts/BatchWritePanel.vue'
import type { LogEntry } from './parts/OperationLogPanel.vue'

const { readOnce } = useReadPointNow()
const { writeOnce } = usePointWrite()

const devices = ref<DeviceConfig[]>([])
const points = ref<PointConfig[]>([])
// 共享调试目标（审查#5）：与参数探测 / 从站扫描等页共用同一台设备选择
const { selectedDeviceId: selectedDevice } = useSharedDebugTarget()
const selectedPoint = ref('')
const devicesLoading = ref(false)
const pointsLoading = ref(false)
// 协议名 -> 能力声明（来自 GET /api/protocols，用于动态渲染操作入口）
const protocolCaps = ref<Record<string, ProtocolCapabilities>>({})

const currentValue = ref<number | string>('—')
const valueLoading = ref(false)
const valueError = ref(false)
const writeLoading = ref(false)
const writeValue = ref<number | null>(null)
const diagLoading = ref(false)

const logs = ref<LogEntry[]>([])

let unmounted = false
// Bug 181: 点位请求序号——快速切换设备时丢弃过期响应，避免旧设备的点位覆盖新选择。
let pointsSeq = 0

const selectedPointObj = computed(() =>
  points.value.find((p) => p.id === selectedPoint.value),
)
const selectedPointType = computed(() => selectedPointObj.value?.data_type || selectedPointObj.value?.protocol || '')

/** 所选设备对应的协议名 */
const selectedProtocol = computed(
  () => devices.value.find((d) => d.id === selectedDevice.value)?.protocol || '',
)
/** 所选协议的能力声明；元数据缺失（未加载/未知协议）时为 null */
const selectedCaps = computed(() => protocolCaps.value[selectedProtocol.value] ?? null)
// 能力门控：元数据可用时严格按声明渲染；不可用时**按不支持处理**。
//
// 口径说明（与 SlaveScan.vue 的 `caps ? caps.supports_scan_slaves : false` 统一）：
// 「能力未知」不等于「能力具备」。写入是对设备的真实副作用操作，若元数据
// 因网络异常/协议未加载而缺失却仍放行写入入口，用户会看到一个必然返回
// `Unsupported` 的按钮，误判为设备故障。故未知一律按「不支持」保守处理。
const capsKnown = computed(() => selectedCaps.value !== null)
const supportsWrite = computed(() => selectedCaps.value?.supports_write ?? false)
const supportsBatchRead = computed(() => selectedCaps.value?.supports_batch_read ?? false)
const supportsScanSlaves = computed(() => selectedCaps.value?.supports_scan_slaves ?? false)
const supportsDiag = computed(() => selectedCaps.value?.supports_diag ?? false)

function addLog(type: LogEntry['type'], tag: string, message: string): LogEntry {
  const time = formatClock()
  const entry: LogEntry = { time, type, tag, message }
  logs.value.unshift(entry)
  if (logs.value.length > 100) logs.value.pop()
  return entry
}

/**
 * 从抓包缓冲取本次读写时间窗内的原始报文（请求/响应 HEX），附到操作记录上。
 * 空态文案承诺了「收发的报文与结果会记录在这里」——没有抓包能力（旧版网关）
 * 或缓冲已被清空时静默跳过，不影响读写主流程。
 */
async function attachFrames(entry: LogEntry, startedAtMs: number, bus: string) {
  try {
    const pkts = await getPackets(30)
    if (unmounted) return
    // 起点放宽 250ms 容忍前后端时钟偏差；按总线过滤（抓包缓冲是全局共享的），
    // 时间正序最多取 4 帧——一对请求/响应足够定位问题
    const hit = pkts
      .filter((p) => (!bus || p.bus === bus) && p.time >= startedAtMs - 250)
      .sort((a, b) => a.time - b.time)
      .slice(0, 4)
    if (hit.length) entry.frames = hit.map((p) => ({ direction: p.direction, hex: p.hex }))
  } catch {
    /* 抓包接口不可用：报文展示是增强能力，缺省不报错 */
  }
}

async function loadDevices() {
  devicesLoading.value = true
  try {
    const data = await listDevices()
    if (unmounted) return
    devices.value = data
    // 共享选择可能已失效（设备被删/配置变更）：本页不替用户改选，仅清空避免展示陈旧 id
    if (selectedDevice.value && !data.some((d) => d.id === selectedDevice.value)) selectedDevice.value = ''
  } catch {
    // HTTP 错误由拦截器处理
  } finally {
    if (!unmounted) devicesLoading.value = false
  }
}

async function loadProtocolCaps() {
  try {
    const list = await listProtocolCapabilities()
    if (unmounted) return
    const map: Record<string, ProtocolCapabilities> = {}
    for (const e of list) map[e.name] = e.capabilities
    protocolCaps.value = map
  } catch {
    // 能力元数据获取失败时保持为空，门控降级为保守放行（不阻断读写功能）
  }
}

async function onDeviceChange(id: string) {
  selectedPoint.value = ''
  currentValue.value = '—'
  valueError.value = false
  // Bug 181: 递增序列号；仅最新一次请求的响应可写入状态，过期响应直接丢弃。
  const seq = ++pointsSeq
  if (!id) { points.value = []; return }
  pointsLoading.value = true
  try {
    const data = await listPoints(id)
    if (unmounted || seq !== pointsSeq) return
    points.value = data
  } catch {
    if (!unmounted && seq === pointsSeq) points.value = []
  } finally {
    if (!unmounted && seq === pointsSeq) pointsLoading.value = false
  }
}

/**
 * BUG-A: 设备选中值可能在「非用户操作」时被写入——共享调试目标
 * （useDebugTargets）在设备列表就绪后会把未选中的状态回退为第一台设备。
 * 此时模板上的 @change 不会触发，点位下拉框就会一直空着（用户看到「无点位可选」）。
 * 统一用 watch 驱动：用户切换、共享目标回退、失效清空都走同一条加载逻辑。
 */
watch(selectedDevice, (id) => { void onDeviceChange(id || '') }, { immediate: true })

function onPointChange() {
  currentValue.value = '—'
  valueError.value = false
  // 切换点位时清空写入值，避免上一轮输入被误用到新点位
  writeValue.value = null
}

async function doRead() {
  if (!selectedPoint.value) return
  valueLoading.value = true
  valueError.value = false
  const ptName = selectedPointObj.value?.name || selectedPoint.value
  const bus = selectedPointObj.value?.bus || ''
  const startedAtMs = Date.now()
  try {
    const outcome = await readOnce(selectedPoint.value)
    if (unmounted) return
    const latency = outcome.latencyMs
    const pt = selectedPointObj.value
    if (outcome.ok) {
      currentValue.value = outcome.value ?? '—'
      const entry = addLog('success', '读取', `${ptName} = ${currentValue.value} (${latency.toFixed(0)}ms)`)
      void attachFrames(entry, startedAtMs, bus)
      if (pt) {
        recordDeviceRead(pt.device_id || pt.id, true, latency)
        recordBusOp(pt.bus, true, latency)
      }
    } else {
      valueError.value = true
      currentValue.value = '—'
      // 现场排障需要"下一步做什么"，而不是只看到底层错误串
      const entry = addLog('danger', '读取', `${ptName} 读取失败: ${withDiagHint(outcome.error)}`)
      void attachFrames(entry, startedAtMs, bus)
      if (pt) {
        recordDeviceRead(pt.device_id || pt.id, false, latency)
        recordBusOp(pt.bus, false, latency, 'read_error')
      }
    }
  } finally {
    if (!unmounted) valueLoading.value = false
  }
}

/**
 * 协议诊断：后端调 Collector::diag（s7comm 读 SZL / bacnet 发 Who-Is / iec104 建链+总召唤 /
 * gpio 读芯片节点）。
 * 每个运行实例（总线）一条结果写入操作记录，便于和读取日志并排比对。
 * 后端对未暂停采集的总线返回 executed=false + 原因（诊断与采集争用同一链路），
 * 这里如实展示原因并提示下一步动作，不自动暂停用户的生产采集。
 */
async function doDiag() {
  const proto = selectedProtocol.value
  if (!proto) return
  diagLoading.value = true
  try {
    const resp = await getProtocolDiag(proto)
    if (unmounted) return
    if (!resp.entries.length) {
      addLog('warning', '协议诊断', `${proto} 没有运行中的采集实例`)
      return
    }
    for (const e of resp.entries) {
      addLog(
        e.executed ? 'success' : 'warning',
        '协议诊断',
        `${proto}@${e.bus}: ${e.detail}`,
      )
    }
    const failed = resp.entries.filter((e) => !e.executed).length
    if (failed) {
      ElMessage.warning(`有 ${failed} 个总线未执行诊断，原因见操作记录`)
    }
  } catch (e) {
    if (!unmounted) {
      const msg = extractErrorMessage(e, '未知错误')
      addLog('danger', '协议诊断', `${proto} 诊断失败: ${msg}`)
      ElMessage.error(`协议诊断失败：${msg}`)
    }
  } finally {
    if (!unmounted) diagLoading.value = false
  }
}

async function doWrite() {
  if (!selectedPoint.value) return
  if (writeValue.value === null || !Number.isFinite(writeValue.value)) {
    ElMessage.warning('请输入有效的数值')
    return
  }
  const ptName = selectedPointObj.value?.name || selectedPoint.value
  const target = writeValue.value
  if (!(await confirmPointWrite([{ id: selectedPoint.value, value: target }], ptName))) return
  if (unmounted) return
  writeLoading.value = true
  const startedAtMs = Date.now()
  const bus = selectedPointObj.value?.bus || ''
  try {
    // 统一写入口：单点写仍传单元素数组，单/批分发由后端决定
    const outcome = await writeOnce(selectedPoint.value, target)
    if (unmounted) return
    if (!outcome.ok) {
      const msg = withDiagHint(outcome.error)
      // 失败同样附报文：请求帧可能已发出（超时/异常码场景只有请求没有响应）
      const entry = addLog('danger', '写入', `${ptName} 写入失败: ${msg}`)
      void attachFrames(entry, startedAtMs, bus)
      ElMessage.error(`写入失败：${msg}`)
      return
    }
    const entry = addLog('success', '写入', `${ptName} ← ${outcome.value ?? target}（结果: ${outcome.status}）`)
    void attachFrames(entry, startedAtMs, bus)
    writeValue.value = null
    await doRead()
  } finally {
    if (!unmounted) writeLoading.value = false
  }
}

/** 页首跳转：批量写入卡片在页面底部，长页下容易错过 */
function scrollToBatchWrite() {
  document.querySelector('.batch-card')?.scrollIntoView({ behavior: 'smooth', block: 'start' })
}

function clearLog() {
  logs.value = []
}

/** 导出操作记录为 CSV：读写测试的收发记录是现场排障证据，支持导出后随工单交付 */
function exportLogs() {
  if (!logs.value.length) return
  // logs 是倒序（最新在前），导出时翻转为时间正序便于阅读
  downloadTableCsv(
    ['时间', '类型', '标签', '内容', '请求帧 HEX', '响应帧 HEX'],
    [...logs.value].reverse().map((l) => [
      l.time,
      l.type,
      l.tag,
      l.message,
      l.frames?.find((f) => f.direction === 'request')?.hex ?? '',
      l.frames?.find((f) => f.direction === 'response')?.hex ?? '',
    ]),
    `tester-log-${new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-')}.csv`,
  )
  ElMessage.success(`已导出 ${logs.value.length} 条操作记录`)
}

onMounted(() => {
  loadDevices()
  loadProtocolCaps()
})
onUnmounted(() => { unmounted = true })
</script>

<style scoped src="./Tester.css"></style>
