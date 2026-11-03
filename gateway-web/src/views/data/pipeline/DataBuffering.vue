<template>
  <!-- 断连续传：只做观测。断开期间的缓存/补传行为完全由网关运行时决定
       （内核常量 BACKPRESSURE_ROWS=100k / MAX_QUEUE_ROWS=1M，见 gateway-core/src/queue/core/limit.rs，
       不在 config.json 中可配），本页不提供任何本地配置项。
       原先的「缓存配置（本地偏好）」表单只写 localStorage、对网关无任何影响（改了没效果），已删除。 -->
  <div class="db">
    <SectionHeader title="断连续传" icon="Connection" />

    <el-row :gutter="16">
      <el-col :span="6"><StatCard label="网关连接" :value="connected ? '在线' : '离线'" :color="connected ? 'var(--text-success)' : 'var(--text-danger)'" /></el-col>
      <el-col :span="6"><StatCard label="队列积压" :value="`${queueLength.toLocaleString()} 条`" /></el-col>
      <el-col :span="6"><StatCard label="在线设备" :value="`${onlineDevices} 台`" /></el-col>
      <!-- mem_usage 取自后端 /proc/meminfo，仅 Linux 提供；Windows 恒为 0，
           显示「0%」会被误读为内存没占用，改为「—」并用 title 说明 -->
      <el-col :span="6"><span :title="memUsage > 0 ? '' : '当前平台不支持采集内存占用（仅 Linux 提供）'"><StatCard label="内存占用" :value="memText" /></span></el-col>
    </el-row>

    <el-card shadow="never" class="section-card">
      <template #header><strong>队列状态</strong> <el-button size="small" link @click="load">刷新</el-button></template>
      <!-- 不设 status（否则 EP 用状态图标替换文字），改用 stroke-color 表达分档，
           format 显示真实条数与占阈值比例，超阈值也能看出超出多少 -->
      <el-progress :percentage="queuePercent" :format="queueFormat" :stroke-color="queueColor" :stroke-width="14" />
      <div v-if="queueLength > 100000" class="over-hint">已超过阈值，将触发自动丢弃过期数据</div>
      <div class="meta">
        <span>当前队列长度：{{ queueLength.toLocaleString() }} 条</span>
        <span>阈值：100,000 条</span>
        <span>CPU 占用：{{ cpuUsage }}%</span>
      </div>
    </el-card>

  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import StatCard from '@/components/StatCard.vue'
import { getStatus } from '@/api/status'

const queueLength = ref(0)
const onlineDevices = ref(0)
const cpuUsage = ref(0)
const memUsage = ref(0)
const connected = ref(true)

const queuePercent = computed(() => Math.min(100, Math.round((queueLength.value / 100000) * 100)))
/** 分档颜色（替代 el-progress 的 status 图标）：>80% 红 / >50% 黄 / 其余绿 */
const queueColor = computed(() =>
  queuePercent.value > 80 ? 'var(--color-danger)' : queuePercent.value > 50 ? 'var(--color-warning)' : 'var(--color-success)',
)
/** 进度条文字：真实条数 / 阈值（真实占比），超阈值也能看出超出多少 */
function queueFormat() {
  return `${queueLength.value.toLocaleString()} / 100,000（${Math.round(queueLength.value / 1000)}%）`
}
/** 内存占用展示：0 表示平台不支持（后端 /proc/meminfo 仅 Linux），显示「—」 */
const memText = computed(() => (memUsage.value > 0 ? `${memUsage.value}%` : '—'))

async function load() {
  try {
    const s = await getStatus()
    queueLength.value = s.queue_length || 0
    onlineDevices.value = s.online_devices || 0
    cpuUsage.value = Math.round(s.cpu_usage || 0)
    memUsage.value = Math.round(s.mem_usage || 0)
    connected.value = true
  } catch {
    connected.value = false
  }
}

onMounted(load)
</script>

<style scoped>
.db { padding: 16px; }
.section-card { margin-bottom: 16px; }
.meta { display: flex; gap: 20px; margin-top: 10px; font-size: 0.8125rem; color: var(--el-text-color-secondary); flex-wrap: wrap; }
.over-hint { margin-top: 8px; font-size: 0.8125rem; color: var(--text-warning); }
</style>