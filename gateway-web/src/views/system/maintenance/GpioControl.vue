<template>
  <div class="gpio-control">
    <SectionHeader title="GPIO 控制" icon="Switch">
      <template #extra>
        <el-tag v-if="status && !platformSupported" size="small" type="info" effect="plain" round>
          当前平台不支持 GPIO
        </el-tag>
        <el-tag v-else-if="status" size="small" :type="allQualityOk ? 'success' : 'warning'" effect="plain" round>
          {{ allQualityOk ? 'DI 输入正常' : 'DI 有异常' }}
        </el-tag>
      </template>
      <el-button type="primary" size="small" @click="load" :loading="loading">
        <el-icon><Refresh /></el-icon> 刷新
      </el-button>
      <el-tooltip content="每 5 秒自动刷新" placement="top">
        <el-switch v-model="autoRefresh" active-text="自动刷新" @change="toggleAuto" />
      </el-tooltip>
    </SectionHeader>
    <span v-if="lastUpdate" class="last-update">最后更新：{{ lastUpdate }}</span>

    <el-alert
      v-if="status && !platformSupported"
      type="warning"
      :closable="false"
      show-icon
      title="当前平台不支持 GPIO 控制"
      description="GPIO 读写依赖 Linux + gpiod（/dev/gpiochip0）。当前平台无该能力，页面不展示任何引脚状态，控制入口已禁用。"
      style="margin-bottom: 12px"
    />

    <!-- DI 输入区 -->
    <el-card shadow="never" class="group-card" v-if="status && platformSupported">
      <template #header>
        <div class="card-head">
          <span class="card-title">数字输入 DI</span>
          <span class="card-sub">读取外部开关/传感器状态（只读）</span>
        </div>
      </template>
      <el-row :gutter="12">
        <el-col :xs="12" :sm="6" v-for="i in 4" :key="`di-${i}`">
          <div class="pin-row">
            <div class="pin-name">DI{{ i - 1 }}</div>
            <div class="pin-indicator" :class="{ on: status.di[i - 1], bad: !status.di_quality[i - 1] }">
              <span class="dot" />
              <span class="pin-value">{{ status.di[i - 1] ? 'HIGH' : 'LOW' }}</span>
            </div>
            <div class="pin-quality" :class="{ ok: status.di_quality[i - 1] }">
              {{ status.di_quality[i - 1] ? '信号正常' : '信号异常' }}
            </div>
          </div>
        </el-col>
      </el-row>
    </el-card>

    <!-- DO 输出区：与 DI 区同样按 capability 门控。
         旧实现只判 `status`，无 GPIO 的平台上仍会渲染 4 路 DO 并全部显示 OFF，
         那是后端默认值而非真实采样 —— 「假状态」比空白更误导（W3a-9）。 -->
    <el-card shadow="never" class="group-card" v-if="status && platformSupported">
      <template #header>
        <div class="card-head">
          <span class="card-title">数字输出 DO</span>
          <span class="card-sub">控制继电器/LED 等输出（点击按钮触发）</span>
        </div>
      </template>
      <el-row :gutter="12">
        <el-col :xs="12" :sm="6" v-for="i in 4" :key="`do-${i}`">
          <div class="pin-row">
            <div class="pin-name">DO{{ i - 1 }}</div>
            <div class="pin-indicator do" :class="{ on: status.do_state[i - 1] }">
              <span class="dot" />
              <span class="pin-value">{{ status.do_state[i - 1] ? 'ON' : 'OFF' }}</span>
            </div>
            <div class="pin-actions">
              <el-button
                size="small"
                :type="status.do_state[i - 1] ? 'success' : 'default'"
                :loading="busyIndex === i - 1"
                :disabled="busyIndex !== null || !platformSupported"
                @click="toggleDo(i - 1)"
              >
                {{ status.do_state[i - 1] ? '拉低' : '拉高' }}
              </el-button>
            </div>
          </div>
        </el-col>
      </el-row>
    </el-card>

    <empty-state v-if="!status && !loading" description="GPIO 状态不可用（可能运行在非 Linux 平台）" />
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { ElMessage } from 'element-plus'
import { Refresh } from '@element-plus/icons-vue'
import { getIoStatus, controlIo, type IoStatus } from '@/api/devices'
import { extractErrorMessage } from '@/utils/errorHandler'
import { logError } from '@/utils/logger'
import { usePolling } from '@/composables/core/usePolling'
import { formatClock } from '@/utils/datetime'

const status = ref<IoStatus | null>(null)
const loading = ref(false)
const autoRefresh = ref(false)
const lastUpdate = ref('')
const busyIndex = ref<number | null>(null)

const allQualityOk = computed(() => {
  if (!status.value) return true
  return status.value.di_quality.every(q => q)
})

/** 当前平台是否具备真实 GPIO 能力；未知时按「支持」处理，避免误禁用 */
const platformSupported = computed(() => status.value?.platform_supported !== false)

async function load() {
  loading.value = true
  try {
    status.value = await getIoStatus()
    lastUpdate.value = formatClock()
  } catch (e) {
    const detail = extractErrorMessage(e, '')
    ElMessage.error(`加载 GPIO 状态失败：${detail || '未知错误'}`)
    logError('gpio', e)
  } finally {
    loading.value = false
  }
}

async function toggleDo(index: number) {
  if (!status.value) return
  const current = status.value.do_state[index]
  const action: 'on' | 'off' = current ? 'off' : 'on'
  busyIndex.value = index
  try {
    const r = await controlIo(index, action)
    // 乐观更新 + 后端结果双保险
    if (status.value) {
      status.value.do_state[index] = r.on
    }
    ElMessage.success(`DO${index} 已${r.on ? '拉高' : '拉低'}`)
  } catch (e) {
    const detail = extractErrorMessage(e, '')
    ElMessage.error(`控制 DO${index} 失败：${detail || '未知错误'}`)
    logError('gpio-ctrl', e)
  } finally {
    busyIndex.value = null
  }
}

const { start: startPoll, stop: stopPoll } = usePolling(load, 5000, { autoStart: false })

function toggleAuto(val: boolean) {
  if (val) startPoll(true)
  else stopPoll()
}

onMounted(load)
</script>

<style scoped>
.gpio-control { padding: 0; }
.last-update { font-size: 0.75rem; color: var(--text-secondary); }

.group-card { margin-bottom: 16px; border-radius: var(--radius-card); border: 1px solid var(--border-base); }
.card-head { display: flex; align-items: baseline; gap: 12px; }
.card-title { font-size: 0.875rem; font-weight: 600; }
.card-sub { font-size: 0.75rem; color: var(--text-secondary); }

.pin-row {
  display: flex; flex-direction: column; gap: 6px;
  padding: 12px 10px;
  border-radius: var(--radius-card);
  background: var(--bg-fill);
  border: 1px solid var(--border-base);
  margin-bottom: 12px;
}
.pin-name { font-size: 0.75rem; color: var(--text-secondary); font-family: 'JetBrains Mono', ui-monospace, monospace; }

.pin-indicator {
  display: flex; align-items: center; gap: 10px;
  padding: 8px 10px;
  border-radius: var(--radius-button);
  background: var(--bg-page);
  border: 1px solid var(--border-base);
}
.pin-indicator .dot {
  width: 10px; height: 10px; border-radius: 50%;
  background: var(--border-lighter);
  transition: background-color 0.15s ease;
}
.pin-indicator.on .dot { background: var(--text-success); box-shadow: 0 0 6px var(--text-success); }
.pin-indicator.bad .dot { background: var(--text-danger); }
.pin-indicator.do.on .dot { background: var(--color-primary); box-shadow: 0 0 6px var(--color-primary); }

.pin-value { font-size: 0.8125rem; font-weight: 600; font-family: 'JetBrains Mono', ui-monospace, monospace; }

.pin-quality { font-size: 0.75rem; color: var(--text-danger); }
.pin-quality.ok { color: var(--text-secondary); }

.pin-actions { display: flex; justify-content: flex-end; }
</style>
