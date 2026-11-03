<template>
  <div class="rl">
    <div class="rl-toolbar">
      <el-button type="primary" size="small" @click="showCreate = true"><el-icon><Plus /></el-icon> 新建规则</el-button>
      <el-button size="small" @click="loadAll"><el-icon><Refresh /></el-icon> 刷新</el-button>
    </div>
    <el-row :gutter="12">
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">总请求数</div><div class="v">{{ st.totalRequests }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">被限流请求数</div><div class="v warn">{{ st.limitedRequests }}</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">平均延迟</div><div class="v">{{ st.avgLatencyMs }} ms</div></el-card></el-col>
      <el-col :span="6"><el-card shadow="never" class="mcard"><div class="l">已启用规则数</div><div class="v">{{ rules.filter(r=>r.enabled).length }}</div></el-card></el-col>
    </el-row>
    <el-card shadow="never" class="section-card">
      <template #header><strong>限流规则</strong></template>
      <DataTable :data="rules" :columns="ruleColumns" row-key="id" stripe empty-text="暂无规则" />
    </el-card>
    <el-card shadow="never" class="section-card">
      <template #header><div style="display:flex;justify-content:space-between"><strong>访问日志</strong><el-button @click="loadLogs" :icon="Refresh">刷新</el-button></div></template>
      <DataTable :data="logs" :columns="logColumns" row-key="id" stripe :max-height="300" empty-text="暂无访问日志" />
    </el-card>

    <!-- 新建规则 -->
    <el-dialog :close-on-click-modal="false" v-model="showCreate" title="新建限流规则" width="480px" :before-close="createGuard.beforeClose">
      <el-form label-width="100px" ref="formRef">
        <el-form-item label="规则名称" prop="name"><el-input v-model="form.name" placeholder="如：设备列表限流" /></el-form-item>
        <el-form-item label="路径" prop="path"><el-input v-model="form.path" placeholder="/api/devices 或 /api/config/*" /></el-form-item>
        <el-form-item label="方法">
          <el-select v-model="form.method" style="width: 100%">
            <el-option v-for="m in ['*', 'GET', 'POST', 'PUT', 'DELETE', 'PATCH']" :key="m" :label="m" :value="m" />
          </el-select>
        </el-form-item>
        <el-form-item label="每分钟"><el-input-number v-model="form.limitPerMinute" :min="1" controls-position="right" /></el-form-item>
        <el-form-item label="每小时"><el-input-number v-model="form.limitPerHour" :min="1" controls-position="right" /></el-form-item>
        <el-form-item label="突发余量"><el-input-number v-model="form.burst" :min="0" controls-position="right" /></el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createGuard.close">取消</el-button>
        <el-button type="primary" :loading="creating" :disabled="creating" @click="create">创建</el-button>
      </template>
    </el-dialog>
  </div>
</template>
<script setup lang="ts">
import { ref, reactive, onMounted, h, nextTick, watch } from 'vue'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
const { showToast } = useErrorDisplay()
import { ElButton, ElIcon, ElMessage, ElTag, ElInputNumber, ElSwitch, type FormInstance } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import {Plus, Refresh} from '@element-plus/icons-vue'
import { listRules, createRule, updateRule, deleteRule, getRecentLogs, getStats, type ApiLimitRule, type ApiLog } from '@/api/apiRateLimit'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn, timeColumn } from '@/utils/commonColumns'
import { setFormFieldErrors, clearFormFieldErrors } from '@/utils/formFieldError'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
const rules = ref<ApiLimitRule[]>([]); const logs = ref<ApiLog[]>([])
const showCreate = ref(false); const creating = ref(false)
const formRef = ref<FormInstance>()
const form = reactive({ name: '', path: '/api/', method: '*', limitPerMinute: 60, limitPerHour: 1000, burst: 20 })

// 新建限流规则弹窗未保存修改守卫：X/ESC/取消时若有改动先确认
const createGuard = useDialogDirtyGuard(showCreate, () => form)

// Dialog 打开/关闭时清错误态（el-dialog 关闭后 DOM 还在，错误残留）
watch(showCreate, (v) => { if (v) nextTick(() => clearFormFieldErrors(formRef.value!)) })

/** el-tag 的 type 需收窄为具体标签色联合，供 h() 渲染使用 */
type TagType = 'primary' | 'success' | 'info' | 'warning' | 'danger'

/**
 * S8: 行内改值不再即时落库。
 * 原实现每个 ElInputNumber / ElSwitch 的 onChange 都直接 `save(row)`，箭头点一下
 * 就发一次写请求、弹一次「规则已更新」——既无法回退，也容易在连续调参时
 * 打出一串半成品配置（限流规则直接影响线上可用性）。
 * 现在：改值只置脏，由行内「保存」按钮确认后才提交；「撤销」恢复原值。
 */
const dirtyIds = ref<Set<string>>(new Set())
function markDirty(row: ApiLimitRule) {
  dirtyIds.value = new Set(dirtyIds.value).add(row.id)
}
function clearDirty(row: ApiLimitRule) {
  const next = new Set(dirtyIds.value)
  next.delete(row.id)
  dirtyIds.value = next
}

// 表格统一走 DataTable：规则与日志行数均有限，默认走 el-table，超阈值自动切虚拟滚动
const ruleColumns: ColumnDef<ApiLimitRule>[] = [
  { key: 'name', title: '规则名称', minWidth: 120, ellipsis: true },
  { key: 'path', title: '路径', minWidth: 160, ellipsis: true },
  { key: 'method', title: '方法', width: 80 },
  {
    key: 'limitPerMinute',
    title: '每分钟',
    width: 100,
    cell: (row) =>
      h(ElInputNumber, {
        modelValue: row.limitPerMinute,
        min: 1,
        size: 'small',
        'onUpdate:modelValue': (v: number | undefined) => { row.limitPerMinute = v ?? 1 },
        onChange: () => markDirty(row),
      }),
  },
  {
    key: 'limitPerHour',
    title: '每小时',
    width: 100,
    cell: (row) =>
      h(ElInputNumber, {
        modelValue: row.limitPerHour,
        min: 1,
        size: 'small',
        'onUpdate:modelValue': (v: number | undefined) => { row.limitPerHour = v ?? 1 },
        onChange: () => markDirty(row),
      }),
  },
  {
    key: 'burst',
    title: '突发',
    width: 80,
    cell: (row) =>
      h(ElInputNumber, {
        modelValue: row.burst,
        min: 0,
        size: 'small',
        'onUpdate:modelValue': (v: number | undefined) => { row.burst = v ?? 0 },
        onChange: () => markDirty(row),
      }),
  },
  {
    key: 'enabled',
    title: '状态',
    width: 70,
    cell: (row) =>
      h(ElSwitch, {
        modelValue: row.enabled,
        'onUpdate:modelValue': (v: unknown) => { row.enabled = Boolean(v) },
        onChange: () => markDirty(row),
      }),
  },
  actionColumn<ApiLimitRule>({
    width: 150,
    fixed: 'right',
    cell: (row) => {
      if (!dirtyIds.value.has(row.id)) {
        return h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => { void remove(row) } }, () => '删除')
      }
      // 有未保存改动时，主操作切换为「保存 / 撤销」
      return h('span', { class: 'rule-actions' }, [
        h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => { void save(row) } }, () => '保存'),
        h(ElButton, { size: 'small', link: true, onClick: () => { void revert(row) } }, () => '撤销'),
      ])
    },
  }),
]

const logColumns: ColumnDef<ApiLog>[] = [
  // S8: 150px 装不下 formatDateTime 的 19 字符（2026/10/29 18:53:30），右侧被截断
  timeColumn<ApiLog>('time', { width: 175 }),
  { key: 'method', title: '方法', width: 70 },
  { key: 'path', title: '路径', minWidth: 140, ellipsis: true },
  {
    key: 'status',
    title: '状态码',
    width: 80,
    cell: (row) =>
      h(
        ElTag,
        { size: 'small', effect: 'dark', round: true, type: (row.status === 200 ? 'success' : row.status === 429 ? 'warning' : 'danger') as TagType },
        () => String(row.status),
      ),
  },
  { key: 'latencyMs', title: '延迟(ms)', width: 80 },
  { key: 'clientIp', title: '客户端IP', width: 130, ellipsis: true },
  {
    key: 'limited',
    title: '限流',
    width: 70,
    cell: (row) =>
      row.limited
        ? h(ElTag, { size: 'small', effect: 'dark', round: true, type: 'danger' }, () => '限流')
        : h('span', null, '-'),
  },
]
const st = ref({ totalRequests: 0, limitedRequests: 0, avgLatencyMs: 0 })
/** 规则加载时的原始副本：供「撤销」回滚到本次会话的已保存状态 */
const ruleSnapshot = new Map<string, ApiLimitRule>()
async function load() {
  try {
    const [r, s] = await Promise.all([listRules(), getStats()])
    rules.value = r
    st.value = s
    ruleSnapshot.clear()
    for (const rule of r) ruleSnapshot.set(rule.id, { ...rule })
    dirtyIds.value = new Set()
  } catch (e) { showToast(e, '加载规则失败') }
}
async function loadLogs() { try { logs.value = await getRecentLogs() } catch (e) { showToast(e, '加载日志失败') } }
async function loadAll() { await Promise.all([load(), loadLogs()]) }
async function save(row: ApiLimitRule) {
  try {
    await updateRule(row)
    ruleSnapshot.set(row.id, { ...row })
    clearDirty(row)
    ElMessage.success('规则已更新')
  } catch (e) { showToast(e, '保存失败') }
}
/** S8: 放弃本行未保存的行内改动，恢复到上次加载/保存时的值 */
function revert(row: ApiLimitRule) {
  const snap = ruleSnapshot.get(row.id)
  if (snap) Object.assign(row, snap)
  clearDirty(row)
}
async function create() {
  // ROI: 必填/格式校验同时高亮涉及的字段（原实现只弹 toast）
  const errors: Record<string, string> = {}
  if (!form.name.trim()) errors.name = '请输入规则名称'
  if (!form.path.startsWith('/')) errors.path = '路径必须以 / 开头'
  if (Object.keys(errors).length > 0) {
    setFormFieldErrors(formRef.value!, errors)
    ElMessage.warning(Object.values(errors)[0])
    return
  }
  clearFormFieldErrors(formRef.value!)
  creating.value = true
  try {
    await createRule({ name: form.name, path: form.path, method: form.method, limitPerMinute: form.limitPerMinute, limitPerHour: form.limitPerHour, burst: form.burst, enabled: true })
    ElMessage.success('已创建')
    showCreate.value = false
    form.name = ''; form.path = '/api/'; form.method = '*'; form.limitPerMinute = 60; form.limitPerHour = 1000; form.burst = 20
    load()
  } catch (e) { showToast(e, '创建限流规则失败') } finally { creating.value = false }
}
async function remove(row: ApiLimitRule) {
  if (!(await confirmDelete(row.name))) return
  try { await deleteRule(row.id); ElMessage.success('已删除'); load() } catch (e) { showToast(e, '删除失败') }
}
onMounted(loadAll)
</script>
<style scoped>
.rl{padding:16px}.v.warn{color: var(--text-warning)}.section-card{margin-top:12px}
.rl-toolbar{display:flex;justify-content:flex-end;gap:8px;margin-bottom:12px}
</style>
