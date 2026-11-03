<template>
  <div class="batch">
    <SectionHeader title="批量命令执行" icon="Operation" />

    <el-card shadow="never" class="section-card">
      <template #header>
        <SectionHeader title="执行流程" icon="Edit" />
      </template>

      <el-steps :active="step" finish-status="success" align-center>
        <el-step v-for="s in stepDefs" :key="s.title">
          <template #title>
            <span>{{ s.title }}</span>
            <el-tooltip :content="s.tip" placement="top">
              <el-icon class="step-tip-icon"><QuestionFilled /></el-icon>
            </el-tooltip>
          </template>
        </el-step>
      </el-steps>

      <div class="step-content">
        <!-- Step 1: 选择操作类型 -->
        <div v-if="step === 0" class="step-body">
          <el-form label-width="100px" style="max-width: 500px; margin: 0 auto">
            <el-form-item label="操作类型">
              <el-radio-group v-model="cmdType">
                <el-radio-button value="read">批量读取</el-radio-button>
                <el-radio-button value="write">批量写入</el-radio-button>
              </el-radio-group>
            </el-form-item>
            <el-form-item v-if="cmdType === 'write'" label="写入值">
              <el-input v-model="writeValue" placeholder="请输入要写入的数值" style="width: 220px" />
            </el-form-item>
          </el-form>
        </div>

        <!-- Step 2: 选择目标 -->
        <div v-else-if="step === 1" class="step-body">
          <div class="target-select">
            <div class="select-col">
              <div class="col-title">待选点位 ({{ availablePoints.length }})</div>
              <div class="select-list">
                <div
                  v-for="p in availablePoints"
                  :key="`${p.deviceId}-${p.pointId}`"
                  class="select-item"
                  role="button"
                  tabindex="0"
                  @click="addTarget(p)"
                  @keydown.enter.prevent="addTarget(p)"
                  @keydown.space.prevent="addTarget(p)"
                >
                  <el-icon class="item-icon"><Right /></el-icon>
                  <span>{{ p.deviceName }} / {{ p.pointName }}</span>
                </div>
              </div>
            </div>
            <div class="select-actions">
              <el-button size="small" @click="addAll"><el-icon><DArrowRight /></el-icon> 全部加入</el-button>
              <el-button size="small" @click="clearAll"><el-icon><Delete /></el-icon> 清空</el-button>
            </div>
            <div class="select-col">
              <div class="col-title">已选目标 ({{ targets.length }})</div>
              <div class="select-list selected">
                <div
                  v-for="t in targets"
                  :key="t.pointId"
                  class="select-item"
                  role="button"
                  tabindex="0"
                  @click="removeTarget(t.pointId)"
                  @keydown.enter.prevent="removeTarget(t.pointId)"
                  @keydown.space.prevent="removeTarget(t.pointId)"
                >
                  <el-icon class="item-icon remove"><Close /></el-icon>
                  <span>{{ t.deviceName }} / {{ t.pointName }}</span>
                </div>
              </div>
            </div>
          </div>
        </div>

        <!-- Step 3: 确认执行 -->
        <div v-else-if="step === 2" class="step-body">
          <el-descriptions :column="2" border size="small" style="max-width: 500px; margin: 0 auto">
            <el-descriptions-item label="操作类型">{{ cmdType === 'read' ? '批量读取' : '批量写入' }}</el-descriptions-item>
            <el-descriptions-item v-if="cmdType === 'write'" label="写入值">{{ writeValue }}</el-descriptions-item>
            <el-descriptions-item label="目标数量">{{ targets.length }} 个点位</el-descriptions-item>
          </el-descriptions>
          <el-alert
            v-if="cmdType === 'write'"
            type="info"
            :closable="false"
            show-icon
            title="写入只对已在点位配置中开启 writable 的点位生效，未开启的会被后端拒绝并显示失败原因"
            style="margin-top: 16px; max-width: 500px; margin-left: auto; margin-right: auto"
          />
          <el-alert
            v-else
            type="warning"
            :closable="false"
            show-icon
            title="确认后将向所有选中目标下发即时读取，会占用总线通讯时间"
            style="margin-top: 16px; max-width: 500px; margin-left: auto; margin-right: auto"
          />
        </div>

        <!-- Step 4: 执行结果 -->
        <div v-else-if="step === 3" class="step-body">
          <div v-if="executing" class="loading-state">
            <el-icon class="is-loading" :size="32"><Loading /></el-icon>
            <p>正在执行批量指令，请稍候...</p>
          </div>
          <template v-else-if="lastResult">
            <el-alert
              :title="`执行完成：成功 ${lastResult.success} 个，失败 ${lastResult.failed} 个`"
              :type="lastResult.failed > 0 ? 'warning' : 'success'"
              :closable="false"
              show-icon
              style="margin-bottom: 12px"
            />
            <DataTable
              :data="lastResult.results"
              :columns="batchResultColumns"
              :row-key="batchResultRowKey"
              stripe
              empty-text="暂无执行结果"
            />
          </template>
          <el-result v-else icon="info" title="等待执行" sub-title="点击下方按钮开始" />
        </div>
      </div>

      <div class="step-actions">
        <el-button v-if="step > 0 && step < 3" @click="step--">上一步</el-button>
        <el-button v-if="step === 0" type="primary" @click="step++" :disabled="!canNextOp">下一步</el-button>
        <el-button v-if="step === 1" type="primary" @click="step++" :disabled="targets.length === 0">下一步</el-button>
        <el-button v-if="step === 2" type="primary" :loading="executing" @click="execute">
          <el-icon><VideoPlay /></el-icon> 执行
        </el-button>
        <el-button v-if="step === 3 && !executing" type="primary" @click="reset">开始新任务</el-button>
      </div>
    </el-card>

    <!-- 历史记录 -->
    <el-card shadow="never" class="section-card" style="margin-top: 16px">
      <template #header>
        <SectionHeader title="执行历史" icon="Clock" />
      </template>
      <DataTable
        :data="history"
        :columns="historyColumns"
        row-key="id"
        stripe
        empty-text="暂无历史记录（本次会话内的执行记录会显示在这里）"
      />
    </el-card>
  </div>
</template>

<script setup lang="ts">
import { h, ref, computed, onMounted } from 'vue'
import { ElMessage, ElTag } from 'element-plus'
import { VideoPlay, Right, Close, Delete, DArrowRight, Loading, QuestionFilled } from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import { usePointWrite } from '@/composables/data/usePointWrite'
import { timeColumn } from '@/utils/commonColumns'

export type BatchCmdType = 'read' | 'write'

export interface BatchTarget {
  deviceId: string
  deviceName: string
  pointId: string
  pointName: string
}

export interface BatchItemResult {
  ok: boolean
  value?: number | string
  /** 单点读取耗时；批量写入无法按点计时，取整批请求的总耗时 */
  latency: number
  error?: string
  deviceName: string
  pointName: string
}

export interface BatchResult {
  id: string
  cmdType: BatchCmdType
  status: 'done'
  createdAt: number
  success: number
  failed: number
  results: BatchItemResult[]
}

// 表格统一走 DataTable；列顺序/宽度与迁移前保持一致
// 执行结果无唯一 ID，用「设备::点位」组合键作为行键
const batchResultRowKey = (row: BatchItemResult) => `${row.deviceName}::${row.pointName}`

const batchResultColumns: ColumnDef<BatchItemResult>[] = [
  { key: 'deviceName', title: '设备', width: 120 },
  { key: 'pointName', title: '点位', width: 120 },
  {
    key: 'ok',
    title: '结果',
    width: 80,
    cell: (row) => h(ElTag, { size: 'small', type: row.ok ? 'success' : 'danger', effect: 'dark' }, () => (row.ok ? '成功' : '失败')),
  },
  { key: 'value', title: '值', width: 120, cell: (row) => (row.ok ? String(row.value ?? '') : '无') },
  { key: 'latency', title: '耗时(ms)', width: 100 },
  { key: 'error', title: '错误信息', minWidth: 120, ellipsis: true, cell: (row) => row.error || '无' },
]

const historyColumns: ColumnDef<BatchResult>[] = [
  timeColumn<BatchResult>('createdAt'),
  {
    key: 'cmdType',
    title: '类型',
    width: 90,
    cell: (row) =>
      h(ElTag, { size: 'small', effect: 'dark', round: true, type: row.cmdType === 'read' ? 'primary' : 'warning' }, () => (row.cmdType === 'read' ? '读取' : '写入')),
  },
  { key: 'success', title: '成功', width: 80 },
  { key: 'failed', title: '失败', width: 80 },
  {
    key: 'status',
    title: '状态',
    width: 90,
    cell: () => h(ElTag, { size: 'small', effect: 'dark', round: true, type: 'success' }, () => '完成'),
  },
]

// 向导步骤：0 选操作 → 1 选目标 → 2 确认 → 3 结果
const step = ref(0)

// 步骤定义：标题 + 悬停说明
const stepDefs = [
  { title: '选择操作', tip: '选择批量读取或批量写入；写入操作需填写写入值' },
  { title: '选择目标', tip: '从待选列表新增目标设备/点位，支持全部加入和清空' },
  { title: '确认执行', tip: '核对操作类型与目标数量，确认后将向所有选中目标发送指令' },
  { title: '执行结果', tip: '查看每个目标的执行结果、返回值、耗时和错误信息' },
]
const cmdType = ref<BatchCmdType>('read')
const writeValue = ref('')
const targets = ref<BatchTarget[]>([])
const availablePoints = ref<BatchTarget[]>([])
const executing = ref(false)
const lastResult = ref<BatchResult | null>(null)
/** 本次会话的执行历史（结果来自真实接口，不做持久化） */
const history = ref<BatchResult[]>([])
const { readOnce } = useReadPointNow()
const { writeBatch } = usePointWrite()

// 步骤0可否下一步：读取总是可，写入需填值
const canNextOp = computed(() => cmdType.value === 'read' || writeValue.value.trim() !== '')

function errMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

async function load() {
  try {
    const devs = await listDevices()
    // 并行拉取每台设备的**全部**点位。批量命令的价值就是一次覆盖几十上百个点位，
    // 旧实现用 slice(0, 5) 只取前 5 个且界面无提示，会让执行结果与实际目标不符。
    const groups = await Promise.all(
      devs.map(async (d): Promise<BatchTarget[]> => {
        try {
          const pts = await listPoints(d.id)
          return pts.map((p) => ({
            deviceId: d.id,
            deviceName: d.name || d.id,
            pointId: p.id,
            pointName: p.name || p.id,
          }))
        } catch {
          // 单台设备点位读取失败不影响其余设备
          return []
        }
      }),
    )
    availablePoints.value = groups.flat()
  } catch { /* ignore */ }
}

function addTarget(p: BatchTarget) {
  if (!targets.value.find((t) => t.pointId === p.pointId)) {
    targets.value.push(p)
  }
}

function removeTarget(pointId: string) {
  targets.value = targets.value.filter((t) => t.pointId !== pointId)
}

function addAll() {
  targets.value = [...availablePoints.value]
}

function clearAll() {
  targets.value = []
}

/** 批量读取：逐点调用 GET /api/points/:id/read，可如实记录每点耗时 */
async function runRead(): Promise<BatchItemResult[]> {
  return Promise.all(
    targets.value.map(async (t) => {
      const outcome = await readOnce(t.pointId)
      return {
        deviceName: t.deviceName,
        pointName: t.pointName,
        ok: outcome.ok,
        value: outcome.ok ? (outcome.value ?? undefined) : undefined,
        latency: Math.round(outcome.latencyMs),
        error: outcome.ok ? undefined : outcome.error,
      }
    }),
  )
}

/** 批量写入：一次 POST /api/points/write，后端按条目数分派（≥2 条按总线分组下发） */
async function runWrite(): Promise<BatchItemResult[]> {
  const value = Number(writeValue.value)
  if (!Number.isFinite(value)) throw new Error('写入值必须是数字')
  const start = Date.now()
  const outcome = await writeBatch(targets.value.map((t) => ({ id: t.pointId, value })))
  const elapsed = Date.now() - start
  if (outcome.error) {
    // 请求级失败：逐条标记失败并带上同一原因，历史记录仍可导出排障
    return targets.value.map((t) => ({
      deviceName: t.deviceName,
      pointName: t.pointName,
      ok: false,
      latency: elapsed,
      error: outcome.error,
    }))
  }
  return targets.value.map((t, i) => {
    const item = outcome.results[i]
    const ok = item?.status === 'ok'
    return {
      deviceName: t.deviceName,
      pointName: t.pointName,
      ok,
      value: ok ? (item?.value ?? value) : undefined,
      latency: elapsed,
      error: ok ? undefined : (item?.error || '写入失败'),
    }
  })
}

async function execute() {
  if (targets.value.length === 0) { ElMessage.warning('请选择目标点位'); return }
  if (cmdType.value === 'write' && !writeValue.value.trim()) { ElMessage.warning('请输入写入值'); return }
  executing.value = true
  // 跳到结果页；执行是同步等待的，无需轮询
  step.value = 3
  const createdAt = Date.now()
  lastResult.value = null
  try {
    const results = cmdType.value === 'write' ? await runWrite() : await runRead()
    const success = results.filter((r) => r.ok).length
    const rec: BatchResult = {
      id: `batch_${createdAt}`,
      cmdType: cmdType.value,
      status: 'done',
      createdAt,
      success,
      failed: results.length - success,
      results,
    }
    lastResult.value = rec
    // 历史只保留本次会话最近 20 条，避免长期占用内存
    history.value.unshift(rec)
    history.value = history.value.slice(0, 20)
  } catch (e) {
    ElMessage.error(`执行失败：${errMessage(e)}`)
    // 执行失败回退到确认页，方便核对参数后重试
    step.value = 2
  } finally {
    executing.value = false
  }
}

function reset() {
  step.value = 0
  targets.value = []
  writeValue.value = ''
  lastResult.value = null
}

onMounted(load)
</script>

<style scoped>
.batch { padding: 0px; }
.section-card { border-radius: var(--radius-card); }
.step-body { padding: 24px 0; min-height: 200px; }
.target-select { display: flex; gap: 12px; align-items: stretch; }
.select-col { flex: 1; border: 1px solid var(--border-light); border-radius: var(--radius-table); overflow: hidden; }
.col-title { padding: 8px 12px; background: var(--bg-fill); font-size: 0.8125rem; font-weight: 600; border-bottom: 1px solid var(--border-light); }
.select-list { max-height: 240px; overflow-y: auto; }
.select-item { padding: 8px 12px; cursor: pointer; display: flex; align-items: center; gap: 6px; border-bottom: 1px solid var(--border-lighter); transition: background .15s; font-size: 0.8125rem; }
.select-item:hover { background: var(--color-primary-lightest); }
.select-list.selected .select-item:hover { background: var(--color-danger-bg); }
.item-icon { color: var(--color-primary); }
.item-icon.remove { color: var(--color-danger); }
.select-actions { display: flex; flex-direction: column; justify-content: center; gap: 8px; }
.step-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; padding-top: 16px; border-top: 1px solid var(--el-border-color-lighter); }
.loading-state { text-align: center; padding: 40px 0; }
.loading-state p { margin-top: 12px; color: var(--text-secondary); }
.step-tip-icon { margin-left: 4px; cursor: help; vertical-align: middle; color: var(--el-text-color-secondary); font-size: 0.875rem; }
</style>
