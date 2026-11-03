<template>
  <div class="sim"><SectionHeader title="从站模拟" icon="Connection" />
    <el-alert
      type="info"
      :closable="false"
      show-icon
      title="在网关本机监听 TCP 端口，作为 Modbus TCP 从站供外部主站读写；寄存器为内存态，重启回默认值。"
      style="margin-bottom: 12px"
    />
    <el-card shadow="never" class="section-card">
      <el-form inline>
        <el-form-item label="从站ID"><el-input-number v-model="slaveId" :min="1" :max="247" :disabled="running" /></el-form-item>
        <el-form-item label="监听端口"><el-input-number v-model="port" :min="1" :max="65535" :disabled="running" /></el-form-item>
        <el-form-item><el-button type="primary" @click="start" :disabled="running" :icon="VideoPlay">启动模拟</el-button><el-button type="danger" @click="stop" :disabled="!running">停止</el-button></el-form-item>
        <el-tag v-if="running" type="success" effect="dark">运行中，从站 {{ status?.slaveId ?? slaveId }}</el-tag>
        <el-tag v-else type="info">已停止</el-tag>
      </el-form>
      <div v-if="running" class="run-meta">
        <span>监听地址：{{ status?.listenAddr ?? '-' }}</span>
        <span>端口：{{ status?.port ?? port }}</span>
        <span>已处理请求：{{ status?.requests ?? 0 }}</span>
        <span>错误数：{{ status?.errors ?? 0 }}</span>
      </div>
      <el-divider />
      <DataTable :data="regs" :columns="regColumns" row-key="addr" stripe empty-text="暂无寄存器数据" />
    </el-card>
  </div>
</template>
<script setup lang="ts">
import { ref, onMounted, onUnmounted, h } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElButton, ElMessage, ElMessageBox } from 'element-plus'
import {VideoPlay} from '@element-plus/icons-vue'
import { getRegisters, updateRegister, startSimulate, stopSimulate, getSimulatorStatus, type SimRegister, type SimulatorStatus } from '@/api/protocolTools'
import { confirmAction } from '@/utils/confirm'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
const slaveId = ref(1); const port = ref(1502); const running = ref(false); const regs = ref<SimRegister[]>([]); const status = ref<SimulatorStatus | null>(null)

// 寄存器表：当前值用 cell 复刻加粗 + 单位，操作列按钮走 cell 渲染
const regColumns: ColumnDef<SimRegister>[] = [
  { key: 'addr', title: '地址', width: 80 },
  { key: 'name', title: '名称', minWidth: 120 },
  {
    key: 'value',
    title: '当前值',
    width: 140,
    cell: (row) => h('span', null, [h('b', { class: { anim: running.value } }, String(row.value)), ` ${row.unit ?? ''}`]),
  },
  { key: 'type', title: '类型', width: 80 },
  actionColumn<SimRegister>({
    width: 100,
    cell: (row) => h(ElButton, { size: 'small', onClick: () => edit(row) }, () => '修改'),
  }),
]
async function load() {
  const [r] = await Promise.all([getRegisters(), refreshStatus()])
  regs.value = r
  if (running.value) startPolling()
}
// 运行中每 2 秒拉取一次 status（端口/请求数/错误数）；组件卸载时清理定时器
let pollTimer: ReturnType<typeof setInterval> | undefined
function startPolling() {
  stopPolling()
  pollTimer = setInterval(refreshStatus, 2000)
}
function stopPolling() {
  if (pollTimer !== undefined) { clearInterval(pollTimer); pollTimer = undefined }
}
async function refreshStatus() {
  try {
    status.value = await getSimulatorStatus()
    running.value = status.value.running
  } catch {
    // 状态轮询失败静默忽略，避免瞬时网络抖动弹提示
  }
}
async function start() {
  const ok = await confirmAction({
    message: `将以从站 ID ${slaveId.value} 在端口 ${port.value} 启动 Modbus TCP 从站，外部主站可连接并读写寄存器（内存态，重启回默认值）。确定启动？`,
    title: '启动从站模拟',
    confirmText: '启动',
  })
  if (!ok) return
  try {
    await startSimulate(slaveId.value, port.value)
    running.value = true
    await refreshStatus()
    startPolling()
    ElMessage.success(`Modbus TCP 从站已启动，监听端口 ${port.value}`)
  } catch (e) { showToast(e, '启动失败') }
}
async function stop() {
  try {
    await stopSimulate()
    running.value = false
    status.value = null
    stopPolling()
    ElMessage.warning('已停止')
  } catch (e) { showToast(e, '停止失败') }
}
async function edit(row: SimRegister) {
  let value: string
  try {
    ({ value } = await ElMessageBox.prompt(`修改 ${row.name} 的值`, '编辑寄存器', { inputValue: String(row.value) }))
  } catch {
    return // 用户取消
  }
  if (!value) return
  try {
    await updateRegister(row.addr, Number(value))
    await load()
    ElMessage.success(`寄存器 ${row.name} 已更新`)
  } catch {
    // PUT 修改寄存器：全局拦截器只留痕，需在此给出可见提示
    ElMessage.error(`修改寄存器 ${row.name} 失败`)
  }
}
onMounted(load)
onUnmounted(stopPolling)
</script>
<style scoped>
.sim{padding:16px}.section-card{margin-bottom:16px}.anim{color:var(--el-color-primary)}
.run-meta{display:flex;flex-wrap:wrap;gap:16px;margin-top:4px;font-size:13px;color:var(--el-text-color-secondary)}
</style>
