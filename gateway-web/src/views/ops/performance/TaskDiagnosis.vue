<template>
  <div class="diag">
    <SectionHeader title="采集诊断" icon="FirstAidKit" />

    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="选择诊断点位" icon="Search" />
      </template>
      <el-form :inline="true">
        <!-- N6: 先按设备筛选，再选点位。原实现只有一个全局点位下拉，点位多时
             根本无法定位到目标设备下的点位。 -->
        <el-form-item label="设备">
          <el-select
            v-model="selectedDevice"
            filterable
            clearable
            placeholder="全部设备"
            style="width: 220px"
            :loading="loadingDevices"
            @change="onDeviceChange"
          >
            <el-option v-for="d in devices" :key="d.id" :label="`${d.name} (${d.id})`" :value="d.id" />
          </el-select>
        </el-form-item>
        <el-form-item label="点位">
          <el-select
            v-model="selectedPoint"
            filterable
            placeholder="选择要诊断的点位 ID"
            style="width: 300px"
            :loading="loadingPoints"
            no-data-text="该设备下还没有点位"
          >
            <el-option v-for="p in points" :key="p.id" :label="`${p.name || p.id} (${p.id})`" :value="p.id" />
          </el-select>
        </el-form-item>
        <el-form-item>
          <el-button type="primary" :loading="diagnosing" :disabled="!selectedPoint" @click="runDiagnosis">
            <el-icon><VideoPlay /></el-icon> 开始诊断
          </el-button>
        </el-form-item>
      </el-form>
      <div v-if="!devices.length && !loadingDevices" class="diag-hint">
        当前网关还没有设备。诊断需要先有设备与点位，请先到
        <router-link class="diag-link" to="/devices">设备列表</router-link> 新增设备。
      </div>
      <div v-else-if="selectedDevice && !loadingPoints && !points.length" class="diag-hint">
        设备「{{ selectedDevice }}」下还没有点位，诊断无从取样。请先到
        <router-link class="diag-link" to="/points">点位管理</router-link> 为该设备新增点位。
      </div>
    </el-card>

    <!-- 诊断结果 -->
    <el-card v-if="result" shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader :title="`诊断结果 - ${selectedPointName}`" icon="DataAnalysis">
          <template #extra>
            <el-tag :type="statusType" effect="dark" round size="small">{{ statusLabel }}</el-tag>
          </template>
          <span class="check-time">{{ formatDateTime(result.lastCheckAt) }}</span>
        </SectionHeader>
      </template>

      <el-steps direction="vertical" :active="activeStep" finish-status="success">
        <el-step
          v-for="(s, i) in result.steps"
          :key="i"
          :status="stepStatus(s.status)"
          :title="s.name"
          :description="s.message"
        >
          <template #icon>
            <el-icon class="step-icon" :class="s.status">
              <CircleCheck v-if="s.status === 'pass'" />
              <CircleClose v-else-if="s.status === 'fail'" />
              <Warning v-else-if="s.status === 'warning'" />
              <Clock v-else />
            </el-icon>
          </template>
        </el-step>
      </el-steps>

      <el-alert
        :title="result.suggestion"
        :type="result.status === 'healthy' ? 'success' : result.status === 'degraded' ? 'warning' : 'error'"
        :closable="false"
        show-icon
        style="margin-top: 16px"
      />
    </el-card>

    <!-- 诊断说明 -->
    <el-card shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="诊断项说明" icon="InfoFilled" />
      </template>
      <el-descriptions :column="1" border size="small">
        <el-descriptions-item label="通信链路">检查串口/TCP 连接是否打开，参数是否匹配</el-descriptions-item>
        <el-descriptions-item label="从站响应">检查从站是否回应，报文格式是否正确，是否超时未响应</el-descriptions-item>
        <el-descriptions-item label="功能码支持">验证设备是否支持该功能码（01/02/03/04/05/06 等）</el-descriptions-item>
        <el-descriptions-item label="地址校验">检查寄存器地址是否在设备支持范围内</el-descriptions-item>
        <el-descriptions-item label="数据解析">验证数据类型（INT16/UINT16/FLOAT 等）和字节序是否正确</el-descriptions-item>
        <el-descriptions-item label="采集频率">检查采集频率是否适时，是否过于频繁导致拥塞</el-descriptions-item>
      </el-descriptions>
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { VideoPlay, CircleCheck, CircleClose, Warning, Clock } from '@element-plus/icons-vue'
import { diagnosePointView as diagnose, type DiagResultView as DiagResult, type DiagStepStatus } from '@/api/diagnostics'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { formatDateTime } from '@/utils/datetime'
import { failureMessage } from '@/utils/errorHandler'
import type { DeviceConfig, PointConfig } from '@/api/types'

const selectedDevice = ref('')
const selectedPoint = ref('')
const devices = ref<DeviceConfig[]>([])
const points = ref<PointConfig[]>([])
const loadingDevices = ref(false)
const loadingPoints = ref(false)
const diagnosing = ref(false)
const result = ref<DiagResult | null>(null)
const activeStep = ref(0)
let unmounted = false
let stepTimer: ReturnType<typeof setTimeout> | null = null

function clearStepTimer() {
  if (stepTimer) {
    clearTimeout(stepTimer)
    stepTimer = null
  }
}

/**
 * N6: 设备筛选 → 点位联动。
 *
 * 诊断只针对单个点位，而点位是「设备 → 点位」两级结构。原实现直接铺开一个全局
 * 点位下拉（且用的是写死的示例点位），现场设备一多就无法定位。这里改为先选设备，
 * 再加载该设备下的真实点位。
 */
async function loadPointsForDevice(deviceId: string) {
  points.value = []
  if (!deviceId) return
  loadingPoints.value = true
  try {
    points.value = await listPoints(deviceId)
  } catch (e) {
    ElMessage.error(failureMessage('加载设备点位', e))
  } finally {
    if (!unmounted) loadingPoints.value = false
  }
}

function onDeviceChange(id: string) {
  selectedPoint.value = ''
  result.value = null
  void loadPointsForDevice(id)
}

onMounted(async () => {
  loadingDevices.value = true
  try {
    devices.value = await listDevices()
    // 只有一台设备时直接选中，省掉一次点击
    if (devices.value.length === 1) {
      selectedDevice.value = devices.value[0]!.id
      await loadPointsForDevice(selectedDevice.value)
    }
  } catch (e) {
    ElMessage.error(failureMessage('加载设备列表', e))
  } finally {
    if (!unmounted) loadingDevices.value = false
  }
})

// 诊断结果里展示的点位名取自当前选中的点位，避免只显示 ID
const selectedPointName = computed(() => {
  const hit = points.value.find((p) => p.id === selectedPoint.value)
  return hit?.name || selectedPoint.value
})
watch(selectedPoint, () => { result.value = null })

const statusType = computed(() => {
  if (!result.value) return 'info'
  return { healthy: 'success', degraded: 'warning', offline: 'danger' }[result.value.status]
})
const statusLabel = computed(() => {
  if (!result.value) return ''
  return { healthy: '健康', degraded: '降级', offline: '离线' }[result.value.status]
})

function stepStatus(s: DiagStepStatus) {
  return { pass: 'success', fail: 'error', warning: 'warning', pending: 'process' }[s]
}

async function runDiagnosis() {
  if (!selectedPoint.value) return
  diagnosing.value = true
  result.value = null
  activeStep.value = 0
  // 调用真实诊断接口（GET /api/diagnose/:point_id），结果完全来自后端
  const r = await diagnose(selectedPoint.value)
  result.value = r
  // 步骤动画仅用于展示后端返回的 steps 逐项进度，不改变诊断结论。
  // 用 setTimeout 链而非 setInterval：到步数即自然结束，重复点「开始诊断」
  // 也不会遗留上一个定时器（interval 会被覆盖而泄漏）。
  clearStepTimer()
  const total = r.steps.length
  let i = 0
  const advance = () => {
    stepTimer = null
    if (unmounted || i >= total) {
      diagnosing.value = false
      return
    }
    i++
    activeStep.value = i
    stepTimer = setTimeout(advance, 400)
  }
  stepTimer = setTimeout(advance, 400)
}
onUnmounted(() => { unmounted = true; clearStepTimer() })
</script>

<style scoped>
.diag { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.check-time { font-size: 0.75rem; color: var(--text-secondary); font-weight: 400; }
/* N6: 无设备时的空态提示与直达入口 */
.diag-hint {
  margin-top: 4px;
  font-size: 0.75rem;
  color: var(--text-secondary);
}
.diag-link {
  margin: 0 2px;
  color: var(--color-primary);
  text-decoration: underline;
}
.step-icon { font-size: 1.25rem; }
.step-icon.pass { color: var(--color-success); }
.step-icon.fail { color: var(--color-danger); }
.step-icon.warning { color: var(--color-warning); }
.step-icon.pending { color: var(--text-placeholder); }
</style>

