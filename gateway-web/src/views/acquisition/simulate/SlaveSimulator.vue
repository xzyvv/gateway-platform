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
        <el-form-item label="从站地址"><el-input-number v-model="slaveId" :min="1" :max="247" :disabled="running" /></el-form-item>
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
import { ref, onMounted, h } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { usePolling } from '@/composables/core/usePolling'
import { ElButton, ElMessage, ElMessageBox } from 'element-plus'
import {VideoPlay} from '@element-plus/icons-vue'
import { getRegisters, updateRegister, startSimulate, stopSimulate, getSimulatorStatus, type SimRegister, type SimulatorStatus } from '@/api/protocolTools'
import { confirmAction } from '@/utils/ui/confirm'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/composables/core/commonColumns'

const { showToast } = useErrorDisplay()
const slaveId = ref(1); const port = ref(1502); const running = ref(false); const regs = ref<SimRegister[]>([]); const status = ref<SimulatorStatus | null>(null)

// 寄存器表：当前值用 cell 复刻加粗 + 单位，操作列按钮走 cell 渲染
const regColumns: ColumnDef<SimRegister>[] = [
  { key: 'addr', title: '寄存器地址', width: 90 },
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
  try {
    const [r] = await Promise.all([getRegisters(), refreshStatus()])
    regs.value = r
    if (running.value) startPoll()
  } catch (e) {
    showToast(e, '加载从站模拟状态失败')
  }
}

/**
 * 运行中每 2 秒拉一次 status（端口/请求数/错误数）。
 *
 * 走 usePolling 而不是手写 setInterval，拿到的是全站统一的：标签页隐藏时停表、
 * KeepAlive 切走时停表、连续失败自动退避（网关重启期间不会每 2 秒打一次）。
 */
const { start: startPoll, stop: stopPoll } = usePolling(refreshStatus, 2000, {
  autoStart: false,
  // 状态轮询失败不弹提示：期间界面仍显示上次状态，退避由 usePolling 负责
  onError: () => {},
})

async function refreshStatus(): Promise<void> {
  const s = await getSimulatorStatus()
  status.value = s
  running.value = s.running
  // 从站已被停掉（被别的会话/重启停掉）时停止轮询，否则会一直空转并显示「运行中」
  if (!s.running) stopPoll()
}

async function start() {
  const ok = await confirmAction({
    message: `将以从站地址 ${slaveId.value} 在端口 ${port.value} 启动 Modbus TCP 从站，外部主站可连接并读写寄存器（内存态，重启回默认值）。确定启动？`,
    title: '启动从站模拟',
    confirmText: '启动',
  })
  if (!ok) return
  try {
    await startSimulate(slaveId.value, port.value)
    running.value = true
    startPoll()
    // 首帧状态拿不到不影响已经起来的模拟器，交给下一拍轮询补齐
    void refreshStatus().catch(() => {})
    ElMessage.success(`Modbus TCP 从站已启动，监听端口 ${port.value}`)
  } catch (e) { showToast(e, '启动从站模拟失败') }
}
async function stop() {
  try {
    await stopSimulate()
    running.value = false
    status.value = null
    stopPoll()
    ElMessage.warning('已停止')
  } catch (e) { showToast(e, '停止从站模拟失败') }
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
  } catch (e) {
    // PUT 修改寄存器：全局拦截器只留痕，需在此给出可见提示（含具体原因）
    showToast(e, `修改寄存器「${row.name}」失败`)
  }
}
onMounted(() => { void load() })
</script>
<style scoped>
.sim{padding:16px}.section-card{margin-bottom:16px}.anim{color:var(--el-color-primary)}
.run-meta{display:flex;flex-wrap:wrap;gap:16px;margin-top:4px;font-size:13px;color:var(--el-text-color-secondary)}
</style>
