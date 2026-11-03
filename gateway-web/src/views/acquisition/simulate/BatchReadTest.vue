<template>
  <div class="brt-page">
    <el-card class="info-card" shadow="never">
      <div class="info-row">
        <el-icon class="info-icon"><Timer /></el-icon>
        <div class="info-text">
          <h3>读取性能测试</h3>
          <p>对点位连续 N 次读取，统计 P50 / P95 / P99 与成功率，评估采集链路稳定性。</p>
        </div>
      </div>
    </el-card>

    <div class="brt-grid">
      <el-card class="control-card" shadow="never">
        <SectionHeader title="选择点位" icon="Cpu" />

        <el-form label-position="top">
          <el-form-item label="设备">
            <el-select
              v-model="selectedDevice"
              placeholder="请选择设备"
              filterable
              aria-label="设备"
              :loading="devicesLoading"
              style="width: 100%"
            >
              <el-option v-for="d in devices" :key="d.id" :label="`${d.name || d.id} (${d.protocol})`" :value="d.id" />
            </el-select>
          </el-form-item>

          <el-form-item label="点位">
            <el-select
              v-model="selectedPoint"
              placeholder="请选择点位"
              filterable
              :disabled="!selectedDevice"
              :loading="pointsLoading"
              style="width: 100%"
            >
              <el-option v-for="p in points" :key="p.id" :label="`${p.name || p.id} (${p.data_type || p.protocol || ''})`" :value="p.id" />
            </el-select>
          </el-form-item>

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

      <el-card class="log-card" shadow="never">
        <SectionHeader title="测试日志" icon="Document">
          <el-button size="small" text @click="logs = []">
            <el-icon><Delete /></el-icon> 清空
          </el-button>
        </SectionHeader>
        <div class="log-list">
          <div v-for="(log, i) in logs" :key="i" class="log-item" :class="log.type">
            <div class="log-time">{{ log.time }}</div>
            <div class="log-content">
              <el-tag size="small" :type="log.type" effect="dark" class="log-tag">{{ log.tag }}</el-tag>
              <span class="log-msg">{{ log.message }}</span>
            </div>
          </div>
          <empty-state v-if="!logs.length" description="执行连续读取后，每次读取的结果会显示在这里" :image-size="80" />
        </div>
      </el-card>
    </div>
  </div>
</template>

<script setup lang="ts">
/** 读取性能测试：对同一个点位连续 N 次读取，统计 P50/P95/P99 与成功率。
 *  从原 Tester.vue 拆出，避免读写测试页单页密度过高。
 */
import { ref, watch, onMounted, onUnmounted } from 'vue'
import { ElMessage } from 'element-plus'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import type { DeviceConfig, PointConfig } from '@/api/types'
import { recordRead as recordDeviceRead } from '@/utils/deviceComms'
import { recordBusOp } from '@/utils/busStats'
import { formatClock } from '@/utils/datetime'
import { useSharedDebugTarget } from '@/composables/acquisition/useDebugTargets'

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
const testCount = ref(20)
const loading = ref(false)
const result = ref<PerfResult | null>(null)
const logs = ref<LogEntry[]>([])

let unmounted = false

function addLog(type: LogEntry['type'], tag: string, message: string) {
  const time = formatClock()
  logs.value.unshift({ time, type, tag, message })
  if (logs.value.length > 100) logs.value.pop()
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

async function onDeviceChange(id: string) {
  selectedPoint.value = ''
  if (!id) { points.value = []; return }
  pointsLoading.value = true
  try {
    const data = await listPoints(id)
    if (unmounted) return
    points.value = data
  } catch {
    if (!unmounted) points.value = []
  } finally {
    if (!unmounted) pointsLoading.value = false
  }
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
    if (unmounted) return
    const outcome = await readOnce(selectedPoint.value)
    latencies.push(outcome.latencyMs)
    if (outcome.ok) success++
    else fail++
    if (ptObj) {
      recordDeviceRead(ptObj.device_id || ptObj.id, outcome.ok, outcome.latencyMs)
      recordBusOp(ptObj.bus, outcome.ok, outcome.latencyMs, outcome.ok ? undefined : 'read_error')
    }
  }
  if (unmounted) return
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
onUnmounted(() => { unmounted = true })
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
.log-list { max-height: 480px; overflow-y: auto; }
.log-item { padding: 0px 10px; border-bottom: 1px solid var(--border-light); }
.log-item:last-child { border-bottom: none; }
.log-time { font-size: 0.6875rem; color: var(--text-secondary); font-family: var(--font-mono); }
.log-content { display: flex; gap: 8px; align-items: center; margin-top: 4px; }
.log-tag { flex-shrink: 0; }
.log-msg { font-size: 0.8125rem; word-break: break-all; }
</style>