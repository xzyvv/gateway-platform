<template>
  <div class="sched">
    <SectionHeader title="计划任务" icon="Timer" />

    <div class="top-bar">
      <el-button type="primary" @click="showCreate = true"><el-icon><Plus /></el-icon> 新建任务</el-button>
      <el-button @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
      <TablePrefsMenu table-id="scheduled-tasks" :columns="taskColumns.map(c => ({ key: c.key, title: c.title }))" />
    </div>

    <DataTable
      :data="tasks"
      :columns="taskColumns"
      :hidden-columns="hiddenColumns"
      row-key="id"
      :loading="loading"
      stripe
      empty-text="暂无计划任务，可新建定时读取/备份/NTP 同步"
      empty-action-text="新建任务"
      @empty-action="showCreate = true"
    />

    <!-- 新建任务对话框 -->
    <el-dialog v-model="showCreate" title="新建计划任务" width="480px" :before-close="createGuard.beforeClose">
      <el-form ref="formRef" :model="form" :rules="rules" label-width="100px" @submit.prevent>
        <el-form-item label="任务名称" prop="name"><el-input v-model="form.name" placeholder="请输入任务名称" /></el-form-item>
        <el-form-item label="任务类型" prop="type">
          <el-select v-model="form.type" style="width: 100%">
            <el-option label="读取点位" value="read_point" />
            <el-option label="读取设备全部点位" value="read_device" />
            <el-option label="备份配置" value="backup_config" />
            <el-option label="同步 NTP" value="sync_ntp" />
          </el-select>
        </el-form-item>
        <el-form-item v-if="needsTarget" label="目标 ID" prop="target">
          <el-input
            v-model="form.target"
            :placeholder="form.type === 'read_point' ? '要读取的点位 ID，如 temp_01' : '要读取的设备 ID，如 dev_001'"
          />
          <div class="form-hint">与「点位管理 / 设备管理」中的 ID 一致；填错会在执行时失败</div>
        </el-form-item>
        <el-form-item label="调度" prop="cron">
          <!-- 预设之外允许直接输入自定义 cron（如 7 分钟一次），但仍按 5 段格式校验 -->
          <el-select
            v-model="form.cron"
            style="width: 100%"
            filterable
            allow-create
            default-first-option
            placeholder="选择预设或直接输入 cron 表达式"
          >
            <el-option v-for="p in presets" :key="p.value" :label="`${p.label} (${p.value})`" :value="p.value" />
          </el-select>
          <div class="form-hint">支持 5 段 cron（分 时 日 月 周），如 <code>*/7 * * * *</code>；时间按网关设备本地时区计算</div>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createGuard.close">取消</el-button>
        <el-button type="primary" :loading="creating" :disabled="creating" @click="create">创建</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, computed, watch, onMounted, h } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElButton, ElMessage, ElSwitch, ElTag, type FormInstance, type FormRules } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import {Plus, Refresh} from '@element-plus/icons-vue'
import InlineEditCell from '@/components/InlineEditCell.vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import { useColumnPrefs } from '@/composables/core/useTablePrefs'
import TablePrefsMenu from '@/components/TablePrefsMenu.vue'
import { formatDateTime } from '@/utils/datetime'
import {
  listTasks, createTask, updateTask, deleteTask, runTaskNow, getCronPresets,
  type ScheduleTask, type TaskType,
} from '@/api/scheduledTasks'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

const tasks = ref<ScheduleTask[]>([])
const loading = ref(false)
const showCreate = ref(false)
/** 任务创建中：网关写接口往返期间禁用按钮，避免重复提交生成多个任务 */
const creating = ref(false)
const presets = getCronPresets()

const typeLabel: Record<TaskType, string> = {
  read_point: '读取点位',
  read_device: '读取设备',
  backup_config: '备份配置',
  sync_ntp: '同步 NTP',
}

/** 任务类型标签：unknown 入参避免 row.type any 污染 */
function taskTypeLabel(t: unknown): string { return typeLabel[t as TaskType] ?? String(t) }

const form = reactive({ name: '', type: 'read_point' as TaskType, target: '', cron: '*/5 * * * *' })
const formRef = ref<FormInstance>()

/** 只有读取类任务才需要目标 ID：此前缺校验，会让后端 400 直接甩到用户面前 */
const needsTarget = computed(() => form.type === 'read_point' || form.type === 'read_device')

/**
 * 5 段 cron 校验（分 时 日 月 周）。
 *
 * 此前只校验「是否 5 段且非空」，`99 99 99 99 99` 这类取值越界的表达式能通过前端，
 * 一路带到后端才被 400 拒绝（后端已严格校验，错误信息为「cron 表达式无效」）。
 * 这里补齐各段取值范围，让用户在表单里就能看到具体哪一段不合法。
 */
const CRON_FIELD_RULES: Array<{ label: string; min: number; max: number }> = [
  { label: '分钟', min: 0, max: 59 },
  { label: '小时', min: 0, max: 23 },
  { label: '日', min: 1, max: 31 },
  { label: '月', min: 1, max: 12 },
  { label: '周', min: 0, max: 6 },
]

/**
 * 校验单个 cron 字段。
 * 支持：通配符 `*`、步长形式（斜杠 + n）、区间 `a-b`、区间带步长、逗号枚举与纯数字。
 */
function cronFieldError(field: string, rule: { label: string; min: number; max: number }): string | null {
  if (field === '*') return null
  for (const part of field.split(',')) {
    const seg = part.trim()
    if (!seg) return `${rule.label}段有空值`
    const rangeMatch = /^(\*|\d+)-(\d+)(?:\/(\d+))?$/.exec(seg)
    if (rangeMatch) {
      const step = rangeMatch[3]
      if (step !== undefined && Number(step) < 1) return `${rule.label}段的步长必须大于 0`
      if (rangeMatch[1] !== '*') {
        const lo = Number(rangeMatch[1])
        const hi = Number(rangeMatch[2])
        if (lo < rule.min || lo > rule.max) return `${rule.label}段必须在 ${rule.min}~${rule.max} 之间`
        if (hi < rule.min || hi > rule.max) return `${rule.label}段必须在 ${rule.min}~${rule.max} 之间`
        if (lo > hi) return `${rule.label}段的起始值大于结束值`
      }
      continue
    }
    const stepMatch = /^\*\/(\d+)$/.exec(seg)
    if (stepMatch) {
      if (Number(stepMatch[1]) < 1) return `${rule.label}段的步长必须大于 0`
      continue
    }
    if (!/^\d+$/.test(seg)) return `${rule.label}段「${seg}」不是合法取值`
    const n = Number(seg)
    if (n < rule.min || n > rule.max) return `${rule.label}段必须在 ${rule.min}~${rule.max} 之间`
  }
  return null
}

function validateCron(_rule: unknown, value: string, callback: (err?: Error) => void) {
  const fields = String(value || '').trim().split(/\s+/)
  if (fields.length !== 5 || fields.some((f) => !f)) {
    callback(new Error('请输入 5 段 cron 表达式，例如 */7 * * * *'))
    return
  }
  for (let i = 0; i < CRON_FIELD_RULES.length; i++) {
    const err = cronFieldError(fields[i]!, CRON_FIELD_RULES[i]!)
    if (err) {
      callback(new Error(err))
      return
    }
  }
  callback()
}

const rules = computed<FormRules>(() => ({
  name: [{ required: true, message: '请输入任务名称', trigger: 'blur' }],
  type: [{ required: true, message: '请选择任务类型', trigger: 'change' }],
  target: needsTarget.value ? [{ required: true, message: '请填写目标 ID', trigger: 'blur' }] : [],
  cron: [
    { required: true, message: '请选择或输入调度表达式', trigger: 'change' },
    { validator: validateCron, trigger: 'change' },
  ],
}))

// 从读取类切到无需目标的类型时清空残留值，避免看不出为什么字段消失了
watch(() => form.type, () => {
  if (!needsTarget.value) form.target = ''
})

// 新建计划任务弹窗未保存修改守卫：X/ESC/取消时若有改动先确认
const createGuard = useDialogDirtyGuard(showCreate, () => form)

/** 列定义：顺序/宽度与原表格一致；任务名称支持双击行内改名 */
const taskColumns: ColumnDef<ScheduleTask>[] = [
  {
    key: 'name',
    title: '任务名称',
    minWidth: 160,
    cell: (row) =>
      h(InlineEditCell, {
        value: row.name,
        label: '任务名称',
        maxlength: 64,
        save: async (v: string | number) => {
          try {
            // 后端为全量更新：带上原行其余字段，仅覆盖名称
            await updateTask({ ...row, name: String(v) })
            row.name = String(v)
            ElMessage.success('任务名称已保存')
            return true
          } catch (e) {
            // 写操作全局静默（拦截器只留痕），这里必须给出可见提示
            showToast(e, '更新计划任务失败')
            return false
          }
        },
      }),
  },
  {
    key: 'type',
    title: '任务类型',
    width: 120,
    cell: (row) => h(ElTag, { size: 'small' }, () => taskTypeLabel(row.type)),
  },
  {
    key: 'cron',
    title: 'Cron 表达式',
    width: 130,
    cell: (row) => h('code', { class: 'cron' }, row.cron),
  },
  {
    key: 'enabled',
    title: '状态',
    width: 80,
    cell: (row) =>
      h(ElSwitch, {
        modelValue: row.enabled,
        'onUpdate:modelValue': (v: unknown) => { row.enabled = Boolean(v) },
        onChange: () => { void onToggle(row) },
      }),
  },
  {
    key: 'lastRunAt',
    title: '上次执行',
    width: 170,
    cell: (row) =>
      row.lastRunAt
        ? h('span', formatDateTime(row.lastRunAt))
        : h('span', '从未'),
  },
  {
    key: 'nextRunAt',
    title: '下次执行',
    width: 170,
    cell: (row) =>
      row.nextRunAt && row.enabled
        ? h('span', formatDateTime(row.nextRunAt))
        : h('span', '从未'),
  },
  {
    // 与前面的启用开关列区分：这一列展示的是上一次执行的结果
    key: 'lastStatus',
    title: '上次结果',
    width: 90,
    cell: (row) =>
      row.lastStatus
        ? h(ElTag, {
          size: 'small',
          type: row.lastStatus === 'success' ? 'success' : 'danger',
          effect: 'dark',
        }, () => (row.lastStatus === 'success' ? '成功' : '失败'))
        : h('span', '从未'),
  },
  { key: 'runCount', title: '执行次数', width: 90, align: 'right' },
  actionColumn<ScheduleTask>({
    width: 140,
    fixed: 'right',
    cell: (row) =>
      h('span', [
        h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => { void runNow(row) } }, () => '立即执行'),
        h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => { void remove(row) } }, () => '删除'),
      ]),
  }),
]

const { hidden: hiddenColumns } = useColumnPrefs('scheduled-tasks')

async function load() {
  loading.value = true
  try { tasks.value = await listTasks() } finally { loading.value = false }
}

async function create() {
  // 前端先做完整校验（含条件必填与 cron 格式），不再让 400 直接弹给用户
  const valid = await formRef.value?.validate().catch(() => false)
  if (!valid) return
  creating.value = true
  try {
    await createTask({
      name: form.name, type: form.type, target: form.target, cron: form.cron, enabled: true,
    })
    ElMessage.success('已创建')
    showCreate.value = false
    form.name = ''; form.target = ''
    formRef.value?.clearValidate()
    load()
  } catch (e) { showToast(e, '创建计划任务失败') } finally { creating.value = false }
}

async function onToggle(t: ScheduleTask) {
  try {
    await updateTask({ ...t })
  } catch (e) {
    showToast(e, '更新失败')
    t.enabled = !t.enabled
  }
}

async function runNow(t: ScheduleTask) {
  try {
    const r = await runTaskNow(t.id)
    if (r.status === 'success') ElMessage.success('执行成功')
    else ElMessage.error(`执行失败：${r.error || '未知错误'}`)
    load()
  } catch (e) {
    showToast(e, '执行失败')
  }
}

async function remove(t: ScheduleTask) {
  if (!(await confirmDelete(t.name))) return
  try {
    await deleteTask(t.id)
    ElMessage.success('已删除')
    load()
  } catch (e) { showToast(e, '删除失败') }
}

onMounted(load)
</script>

<style scoped>
.sched { padding: 0px; }
.top-bar { display: flex; gap: 8px; margin-bottom: 12px; }
.cron { font-family: monospace; background: var(--bg-fill); padding: 2px 6px; border-radius: var(--radius-small); font-size: 0.75rem; }
.form-hint { margin-top: 4px; font-size: 0.72rem; color: var(--text-secondary); line-height: 1.4; }
</style>
