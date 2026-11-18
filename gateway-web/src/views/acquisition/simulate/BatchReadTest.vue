<template>
  <div class="brt-page">
    <el-card class="info-card" shadow="never">
      <div class="info-row">
        <el-icon class="info-icon"><Timer /></el-icon>
        <div class="info-text">
          <h3>读取性能</h3>
          <p>对点位连续 N 次读取，统计 P50 / P95 / P99 与成功率，评估采集链路稳定性。</p>
        </div>
      </div>
    </el-card>

    <div class="brt-grid">
      <el-card class="control-card" shadow="never">
        <SectionHeader title="选择点位" icon="Cpu" />

        <!-- 与「点位读写」页共用同一个选择器：能力徽标、设备/点位标签格式、aria-label 三处口径一致，
             避免同一 Hub 内两套选择器各写一份、行为还对不上 -->
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

        <el-form label-position="top">
          <el-form-item label="测试次数">
            <el-input-number v-model="testCount" :min="1" :max="1000" :step="10" controls-position="right" style="width: 100%" />
          </el-form-item>
        </el-form>

        <div class="action-row">
          <el-button type="primary" :disabled="!selectedPoint" :loading="loading" @click="doBatchRead">
            <el-icon><VideoPlay /></el-icon> 开始测试
          </el-button>
          <el-button :disabled="!result" @click="result = null">
            <el-icon><Refresh /></el-icon> 清空结果
          </el-button>
        </div>

        <div v-if="result" class="result-block">
          <el-divider />
          <div class="result-summary">
            <el-tag :type="result.successRate >= 95 ? 'success' : result.successRate >= 80 ? 'warning' : 'danger'" size="large" effect="dark">
              成功率 {{ result.successRate }}%
            </el-tag>
            <span class="result-meta">{{ result.success }} 成功 / {{ result.fail }} 失败 / 共 {{ result.count }} 次</span>
          </div>
          <el-descriptions :column="3" border size="small" class="result-desc">
            <el-descriptions-item label="P50">{{ result.p50 }}ms</el-descriptions-item>
            <el-descriptions-item label="P95">{{ result.p95 }}ms</el-descriptions-item>
            <el-descriptions-item label="P99">{{ result.p99 }}ms</el-descriptions-item>
            <el-descriptions-item label="最小">{{ result.min }}ms</el-descriptions-item>
            <el-descriptions-item label="平均">{{ result.avg }}ms</el-descriptions-item>
            <el-descriptions-item label="最大">{{ result.max }}ms</el-descriptions-item>
          </el-descriptions>
        </div>
      </el-card>

      <LogPanel
        class="log-card"
        title="测试日志"
        icon="Document"
        :empty="logs.length === 0"
        empty-text="执行连续读取后，每次读取的结果会显示在这里"
        copyable
        exportable
        clearable
        @copy-all="copyAllLogs"
        @export="exportLogs"
        @clear="logs = []"
      >
        <div v-for="(log, i) in logs" :key="i" class="log-item" :class="log.type">
          <div class="log-time">{{ log.time }}</div>
          <div class="log-content">
            <el-tag size="small" :type="log.type" effect="dark" class="log-tag">{{ log.tag }}</el-tag>
            <span class="log-msg">{{ log.message }}</span>
          </div>
        </div>
      </LogPanel>
    </div>
  </div>
</template>

<script setup lang="ts">
/** 读取性能：对同一个点位连续 N 次读取，统计 P50/P95/P99 与成功率。
 *  从原 Tester.vue 拆出，避免读写测试页单页密度过高。
 */
import { ref, watch, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import type { DeviceConfig, PointConfig } from '@/api/types'
import { recordRead as recordDeviceRead } from '@/utils/deviceComms'
import { formatClock } from '@/utils/datetime'
import { useSharedDebugTarget } from '@/composables/acquisition/useDebugTargets'
import { useProtocolGates } from '@/composables/acquisition/useProtocolGates'
import DevicePointSelector from './parts/DevicePointSelector.vue'
import LogPanel from '@/components/LogPanel.vue'
import { copyText } from '@/utils/clipboard'
import { csvFilename, downloadTableCsv } from '@/utils/exportCsv'
import { useRequestGuard } from '@/composables/core/useRequestGuard'
import { useUnmountedGuard } from '@/composables/core/useUnmountedGuard'

interface PerfResult {
  count: number
  success: number
  fail: number
  successRate: number
  p50: number
  p95: number
  p99: number
  min: number
  avg: number
  max: number
}
interface LogEntry {
  time: string
  type: 'info' | 'success' | 'warning' | 'danger'
  tag: string
  message: string
}

const { readOnce } = useReadPointNow()

const devices = ref<DeviceConfig[]>([])
const points = ref<PointConfig[]>([])
// 共享调试目标（审查#5）：与参数探测 / 从站扫描等页共用同一台设备选择
const { selectedDeviceId: selectedDevice } = useSharedDebugTarget()
const selectedPoint = ref('')
const devicesLoading = ref(false)
const pointsLoading = ref(false)
// 协议能力门控（口径见 useProtocolGates）：驱动选择器上的能力徽标
const { selectedProtocol, capsKnown, supportsWrite, supportsBatchRead, supportsScanSlaves, supportsDiag } =
  useProtocolGates(devices, selectedDevice)
const testCount = ref(20)
const loading = ref(false)
const result = ref<PerfResult | null>(null)
const logs = ref<LogEntry[]>([])

const { isUnmounted } = useUnmountedGuard()
/**
 * Bug 181：点位请求序号——快速切换设备时丢弃过期响应。
 * 本页此前没有这层保护，旧设备的点位列表可能在用户已切到新设备后回写，
 * 导致下拉里出现不属于当前设备的点位。
 */
// 请求序号守卫：只接受最新一次请求的结果（旧响应不得覆盖新选择）
const { next: nextRequest, isCurrent } = useRequestGuard()

/**
 * 复制全部测试日志：一轮 1000 次读取跑完，P50/P99 的结论通常要贴进工单，
 * 逐条复制不现实；导出 CSV 又太重（对方只是想看几行）。
 */
function copyAllLogs() {
  if (!logs.value.length) return
  const text = [...logs.value]
    .reverse()
    .map((l) => `[${l.time}] ${l.tag} ${l.message}`)
    .join('\n')
  void copyText(text, `已复制 ${logs.value.length} 条测试日志`)
}

/** 导出测试日志为 CSV：交付性能报告时随附原始逐次结果 */
function exportLogs() {
  if (!logs.value.length) return
  downloadTableCsv(
    ['时间', '类型', '标签', '内容'],
    [...logs.value].reverse().map((l) => [l.time, l.type, l.tag, l.message]),
    csvFilename('batch-read-log'),
  )
  ElMessage.success(`已导出 ${logs.value.length} 条测试日志`)
}

function addLog(type: LogEntry['type'], tag: string, message: string) {
  const time = formatClock()
  logs.value.unshift({ time, type, tag, message })
  if (logs.value.length > 100) logs.value.pop()
}

async function loadDevices() {
  devicesLoading.value = true
  try {
    const data = await listDevices()
    if (isUnmounted()) return
    devices.value = data
    // 共享选择可能已失效（设备被删/配置变更）：本页不替用户改选，仅清空避免展示陈旧 id
    if (selectedDevice.value && !data.some((d) => d.id === selectedDevice.value)) selectedDevice.value = ''
  } catch {
    // HTTP 错误由拦截器处理
  } finally {
    if (!isUnmounted()) devicesLoading.value = false
  }
}

async function onDeviceChange(id: string) {
  selectedPoint.value = ''
  // 递增序号：仅最新一次请求的响应可写入状态，过期响应直接丢弃（Bug 181）
  const seq = nextRequest()
  if (!id) { points.value = []; return }
  pointsLoading.value = true
  try {
    const data = await listPoints(id)
    if (!isCurrent(seq)) return
    points.value = data
  } catch {
    if (isCurrent(seq)) points.value = []
  } finally {
    if (isCurrent(seq)) pointsLoading.value = false
  }
}

/**
 * 切换点位时清空上一轮结果：统计块里不带点位名，留着上一个点位的
 * P50/P95/P99 会被当成新点位的成绩。
 */
function onPointChange() {
  result.value = null
}

/**
 * BUG-A: 设备选中值会在设备列表就绪后被共享调试目标自动回退为第一台设备，
 * 该写入不触发模板 @change，点位下拉框会一直为空。统一用 watch 驱动加载。
 */
watch(selectedDevice, (id) => { void onDeviceChange(id || '') }, { immediate: true })

async function doBatchRead() {
  if (!selectedPoint.value) return
  const N = testCount.value
  const ptObj = points.value.find((p) => p.id === selectedPoint.value)
  const ptName = ptObj?.name || selectedPoint.value
  loading.value = true
  result.value = null
  addLog('info', '开始', `对 ${ptName} 发起 ${N} 次读取`)

  const latencies: number[] = []
  let success = 0
  let fail = 0
  for (let i = 0; i < N; i++) {
    if (isUnmounted()) return
    const outcome = await readOnce(selectedPoint.value)
    latencies.push(outcome.latencyMs)
    if (outcome.ok) success++
    else fail++
    if (ptObj) {
      recordDeviceRead(ptObj.device_id || ptObj.id, outcome.ok, outcome.latencyMs)
    }
  }
  if (isUnmounted()) return
  const sorted = [...latencies].sort((a, b) => a - b)
  const pct = (p: number): number => sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))] ?? 0
  const sum = sorted.reduce((a, b) => a + b, 0)
  result.value = {
    count: N,
    success, fail,
    successRate: Math.round((success / N) * 100),
    p50: Math.round(pct(50)),
    p95: Math.round(pct(95)),
    p99: Math.round(pct(99)),
    min: Math.round(sorted[0] ?? 0),
    avg: Math.round(sum / sorted.length),
    max: Math.round(sorted[sorted.length - 1] ?? 0),
  }
  loading.value = false
  const r = result.value
  addLog(r.fail === 0 ? 'success' : 'warning', '完成',
    `${ptName}: P50=${r.p50}ms P99=${r.p99}ms 成功率=${r.successRate}%`)
  if (r.successRate < 80) {
    ElMessage.warning(`成功率仅 ${r.successRate}%，请检查链路`)
  }
}

onMounted(loadDevices)
</script>

<style scoped>
.brt-page { padding: 0; }
.info-card { margin-bottom: 16px; }
.info-row { display: flex; gap: 12px; align-items: flex-start; }
.info-icon { font-size: 1.5rem; color: var(--el-color-primary); }
.info-text h3 { margin: 0 0 4px; font-size: 0.9375rem; }
.info-text p { margin: 0; color: var(--text-secondary); font-size: 0.8125rem; line-height: 1.6; }
.brt-grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }
@media (max-width: 1200px) { .brt-grid { grid-template-columns: 1fr; } }
.control-card { min-height: 420px; }
.action-row { display: flex; gap: 8px; margin-top: 12px; }
.result-block { margin-top: 16px; }
.result-summary { display: flex; align-items: center; gap: 12px; margin-bottom: 12px; }
.result-meta { color: var(--text-secondary); font-size: 0.8125rem; }
.result-desc { margin-top: 8px; }
.log-card { min-height: 420px; }
.log-item { padding: 0px 10px; border-bottom: 1px solid var(--border-light); }
.log-item:last-child { border-bottom: none; }
.log-time { font-size: 0.6875rem; color: var(--text-secondary); font-family: var(--font-mono); }
.log-content { display: flex; gap: 8px; align-items: center; margin-top: 4px; }
.log-tag { flex-shrink: 0; }
.log-msg { font-size: 0.8125rem; word-break: break-all; }
</style>