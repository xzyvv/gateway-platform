<template>
  <div class="wd">
    <el-card shadow="never" class="section-card">
      <template #header>
        <strong>服务状态</strong>
        <el-button size="small" link @click="load" style="float:right">刷新</el-button>
      </template>
      <DataTable
        :data="services"
        :columns="serviceColumns"
        row-key="name"
        :loading="loading"
        stripe
        empty-text="暂无服务"
      />
    </el-card>

    <el-card shadow="never" class="section-card">
      <template #header>
        <strong>自愈执行器</strong>
        <el-tag size="small" :type="heal.enabled ? 'success' : 'info'" effect="dark" style="margin-left:8px">
          {{ heal.enabled ? '已启用' : '未启用' }}
        </el-tag>
        <el-button size="small" link @click="loadHeal" style="float:right">刷新</el-button>
      </template>
      <el-form label-width="170px" size="small">
        <el-form-item label="启用自愈执行器">
          <el-switch v-model="heal.enabled" />
          <span class="hint">（opt-in，默认关闭；关闭时存量部署行为零变化）</span>
        </el-form-item>
        <el-form-item label="评估周期(秒)">
          <el-input-number v-model="heal.check_interval_secs" :min="1" controls-position="right" />
        </el-form-item>
        <el-form-item label="触发防抖次数">
          <el-input-number v-model="heal.debounce_k" :min="1" controls-position="right" />
        </el-form-item>
        <el-form-item label="动作冷却(秒)">
          <el-input-number v-model="heal.cooldown_secs" :min="0" controls-position="right" />
        </el-form-item>
        <el-form-item label="限速窗口(秒)">
          <el-input-number v-model="heal.window_secs" :min="1" controls-position="right" />
        </el-form-item>
        <el-form-item label="窗口内最大动作数">
          <el-input-number v-model="heal.max_actions_per_window" :min="1" controls-position="right" />
        </el-form-item>
        <el-form-item label="熔断失败次数">
          <el-input-number v-model="heal.fail_streak_to_fuse" :min="1" controls-position="right" />
        </el-form-item>
        <el-form-item>
          <el-button type="primary" :loading="saving" @click="saveHeal">保存</el-button>
          <el-button @click="loadHeal">重置</el-button>
        </el-form-item>
      </el-form>
      <el-alert
        title="自愈执行器为 opt-in 功能：修改后需重启网关生效（运行中切换不会即时启停执行器）"
        type="warning"
        :closable="false"
        show-icon
      />
    </el-card>

    <el-card shadow="never" class="section-card">
      <template #header><strong>自愈事件历史</strong></template>
      <el-timeline v-if="events.length">
        <el-timeline-item
          v-for="e in events"
          :key="e.id"
          :timestamp="e.time ? formatDateTime(e.time * 1000) : '-'"
          :type="e.result === 'success' ? 'success' : 'danger'"
        >
          <div class="evt">
            <el-tag size="small" effect="dark">{{ ACTION_LABEL[e.action] }}</el-tag>
            <strong>{{ e.target }}</strong>
            <span class="reason">原因: {{ e.reason }}</span>
            <span v-if="e.detail" class="detail">详情: {{ e.detail }}</span>
          </div>
        </el-timeline-item>
      </el-timeline>
      <EmptyState v-else description="启用自愈执行器后，触发的自愈动作会按时间线呈现" :image-size="60" />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, h } from 'vue'
import { ElButton, ElMessage } from 'element-plus'
import type { ColumnDef } from '@/components/DataTable.vue'
import {
  listServices,
  listHealEvents,
  restartService,
  ACTION_LABEL,
  STATUS_LABEL,
  STATUS_TYPE,
  type ServiceInfo,
  type HealEvent,
} from '@/api/watchdog'
import { getConfig, postConfig } from '@/api/config'
import type { SelfHealConfig } from '@/api/types'
import { formatDateTime } from '@/utils/datetime'
import { actionColumn, tagColumn } from '@/utils/commonColumns'

// 自愈配置缺省值（与后端 SelfHealConfig::default 保持一致：默认关闭）
const DEFAULT_HEAL: SelfHealConfig = {
  enabled: false,
  check_interval_secs: 15,
  debounce_k: 3,
  cooldown_secs: 300,
  window_secs: 3600,
  max_actions_per_window: 10,
  fail_streak_to_fuse: 3,
}

const services = ref<ServiceInfo[]>([])
const events = ref<HealEvent[]>([])
const heal = ref<SelfHealConfig>({ ...DEFAULT_HEAL })
const loading = ref(false)
const saving = ref(false)
const restarting = ref<string | null>(null)
let unmounted = false
let restartTimer: ReturnType<typeof setTimeout> | null = null

async function load() {
  loading.value = true
  try {
    const [svcs, evts] = await Promise.all([listServices(), listHealEvents()])
    services.value = svcs
    events.value = evts
  } catch {
    ElMessage.error('加载看门狗状态失败')
  } finally {
    loading.value = false
  }
}

/** 读取 [self_heal] 配置节（缺省即默认关闭） */
async function loadHeal() {
  try {
    const cfg = await getConfig()
    heal.value = { ...DEFAULT_HEAL, ...(cfg.self_heal || {}) }
  } catch {
    ElMessage.error('加载自愈配置失败')
  }
}

/** 保存 [self_heal] 配置节（走 getConfig → 改字段 → postConfig 全量回写） */
async function saveHeal() {
  saving.value = true
  try {
    const cfg = await getConfig()
    cfg.self_heal = { ...heal.value }
    await postConfig(cfg)
    ElMessage.success('自愈配置已保存，重启网关后生效')
  } catch {
    ElMessage.error('保存自愈配置失败')
  } finally {
    saving.value = false
  }
}

async function restart(name: string) {
  restarting.value = name
  try {
    await restartService(name)
    ElMessage.success(`已触发 ${name} 服务重启`)
    restartTimer = setTimeout(() => { if (!unmounted) load() }, 1500)
  } catch {
    ElMessage.error('重启失败')
  } finally {
    restarting.value = null
  }
}

function formatUptime(s: number) {
  const d = Math.floor(s / 86400)
  const h = Math.floor((s % 86400) / 3600)
  const m = Math.floor((s % 3600) / 60)
  return `${d}天${h}时${m}分`
}

// 服务状态表统一走 DataTable：列顺序/宽度与迁移前保持一致
const serviceColumns: ColumnDef<ServiceInfo>[] = [
  { key: 'name', title: '服务', minWidth: 160 },
  tagColumn<ServiceInfo>('status', {
    title: '状态',
    width: 100,
    type: (row) => STATUS_TYPE[row.status],
    label: (row) => STATUS_LABEL[row.status],
  }),
  { key: 'pid', title: 'PID', width: 80 },
  {
    key: 'uptime',
    title: '运行时长',
    width: 120,
    cell: (row) => (row.uptime > 0 ? formatUptime(row.uptime) : '-'),
  },
  {
    key: 'cpu',
    title: 'CPU(%)',
    width: 90,
    align: 'right',
    cell: (row) => (row.cpu ? row.cpu.toFixed(1) : '-'),
  },
  { key: 'mem', title: '内存(MB)', width: 100, align: 'right' },
  { key: 'restarts', title: '重启次数', width: 90, align: 'right' },
  actionColumn<ServiceInfo>({
    width: 100,
    cell: (row) =>
      h(
        ElButton,
        {
          size: 'small',
          type: 'warning',
          link: true,
          loading: restarting.value === row.name,
          onClick: () => restart(row.name),
        },
        () => '重启',
      ),
  }),
]

onMounted(() => { load(); loadHeal() })
onUnmounted(() => { unmounted = true; if (restartTimer) clearTimeout(restartTimer) })
</script>

<style scoped>
.wd { padding: 16px; }
.section-card { margin-bottom: 16px; }
.evt { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
.reason { color: var(--el-text-color-secondary); font-size: 0.8125rem; }
.detail { color: var(--text-success); font-size: 0.8125rem; }
.hint { margin-left: 8px; color: var(--el-text-color-secondary); font-size: 0.75rem; }
</style>