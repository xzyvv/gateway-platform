<template>
  <div class="pc">
    <SectionHeader title="协议转换" icon="Share" />
    <p class="hint">常用值变换模板（原值 / 线性 / 枚举），免脚本编码；按「源点位 ID」匹配采集点，转换结果随既有上行通道上送。</p>
    <div class="top-bar">
      <el-button type="primary" @click="openCreate"><el-icon><Plus /></el-icon> 新建转换规则</el-button>
      <el-button @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
    </div>
    <DataTable
      :data="rules"
      :columns="ruleColumns"
      row-key="id"
      stripe
      :loading="loading"
      empty-text="暂无转换规则，新建后按源点位 ID 匹配采集点"
      empty-action-text="新建转换规则"
      @empty-action="openCreate"
    />

    <el-dialog v-model="showForm" :title="isEdit ? '编辑转换规则' : '新建转换规则'" width="560px" :before-close="formGuard.beforeClose">
      <el-form ref="formRef" :model="form" :rules="formRules" label-width="100px">
        <el-form-item label="规则名称" prop="name"><el-input v-model="form.name" placeholder="请输入规则名称" /></el-form-item>
        <el-form-item label="源协议">
          <!-- 源协议属于采集侧，与点位/设备共用同一份协议注册表（useProtocol） -->
          <ProtocolSelector v-model="form.sourceProto" aria-label="源协议" />
        </el-form-item>
        <el-form-item label="源点位 ID" prop="sourcePointId">
          <!-- 运行时按 ID 精确匹配：手填错一个字符，规则会静默不生效，故改为从配置点位里选 -->
          <el-select
            v-model="form.sourcePointId"
            filterable
            allow-create
            default-first-option
            :loading="pointsLoading"
            placeholder="选择采集点位（运行时按 ID 匹配）"
            style="width:100%"
            @change="onSourcePointChange"
          >
            <el-option v-for="o in sourcePointOptions" :key="o.value" :label="o.label" :value="o.value" />
          </el-select>
          <div v-if="!pointsLoading && points.length === 0" class="preview-tip">
            未读取到点位清单，请先在「设备 → 点位」中创建采集点位；也可直接输入已有 ID。
          </div>
        </el-form-item>
        <el-form-item label="源点位名称"><el-input v-model="form.sourcePointName" placeholder="展示名称（可选）" /></el-form-item>
        <el-form-item label="目标协议">
          <!-- 目标协议是上行通道（MQTT / Modbus TCP 等），与南向协议注册表不是同一命名空间，保留独立清单 -->
          <el-select v-model="form.targetProto" style="width:100%"><el-option v-for="(l,k) in PROTO_LABEL" :key="k" :label="l" :value="k" /></el-select>
        </el-form-item>
        <el-form-item label="目标标签"><el-input v-model="form.targetTag" placeholder="如 factory/line1/temp" /></el-form-item>
        <el-form-item label="转换方式">
          <el-radio-group v-model="form.transform">
            <el-radio value="raw">原值</el-radio>
            <el-radio value="linear">线性变换</el-radio>
            <el-radio value="enum">枚举</el-radio>
          </el-radio-group>
        </el-form-item>
        <template v-if="form.transform === 'linear'">
          <el-form-item label="缩放系数"><el-input-number v-model="form.scale" :step="0.001" controls-position="right" /></el-form-item>
          <el-form-item label="偏移量"><el-input-number v-model="form.offset" :step="0.001" controls-position="right" /></el-form-item>
        </template>
        <template v-if="form.transform === 'enum'">
          <el-form-item label="枚举映射" prop="enumMap">
            <div v-for="(m, i) in form.enumMap" :key="i" style="display:flex;gap:8px;margin-bottom:6px">
              <el-input v-model="m.key" placeholder="值" style="width:100px" />
              <el-input v-model="m.label" placeholder="标签" style="width:160px" />
              <el-button type="danger" link @click="form.enumMap!.splice(i,1)">删除</el-button>
            </div>
            <el-button size="small" link @click="form.enumMap!.push({key:'',label:''})">新增</el-button>
          </el-form-item>
        </template>
        <el-form-item label="状态"><el-switch v-model="form.enabled" /></el-form-item>
        <el-form-item label="本地试跑">
          <div class="preview">
            <el-input v-model="previewRaw" placeholder="输入采集原始值" style="width: 160px" />
            <span class="preview-arrow">→</span>
            <strong class="preview-out">{{ previewResult }}</strong>
          </div>
          <div class="preview-tip">
            仅在本页按当前表单参数即时换算，用于确认规则填得对不对；网关运行时以后端同一口径执行，本页不发请求、也不改动上行数据。
          </div>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="formGuard.close">取消</el-button>
        <el-button type="primary" :loading="submitting" :disabled="submitting" @click="save">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, computed, onMounted, nextTick, h, Fragment } from 'vue'
import { ElMessage, ElButton, ElSwitch, ElTag, type FormInstance, type FormRules } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import {Plus, Refresh} from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import { listConvertRules, createConvertRule, updateConvertRule, deleteConvertRule, toggleConvertRule, PROTO_LABEL, type ConvertRule } from '@/api/protocolTools'
import ProtocolSelector from '@/components/industrial/ProtocolSelector.vue'
import { getConfig } from '@/api/config'
import type { PointConfig } from '@/api/types'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'

const rules = ref<ConvertRule[]>([])
/** 可选点位：规则运行时按 source_point_id 精确匹配，列出真实 ID 避免手填错字后静默不生效 */
const points = ref<PointConfig[]>([])
const pointsLoading = ref(false)
async function loadPoints() {
  pointsLoading.value = true
  try {
    const cfg = await getConfig()
    points.value = cfg.points || []
  } catch {
    // 读不到点位不影响编辑规则其余字段，退回手动输入 ID
    points.value = []
  } finally {
    pointsLoading.value = false
  }
}
const { showToast } = useErrorDisplay()
const loading = ref(false)
const showForm = ref(false)
const isEdit = ref(false)
const editingId = ref('')
/** 规则保存中：网关写接口往返期间禁用保存按钮，避免重复提交生成多条规则 */
const submitting = ref(false)
interface ConvertForm {
  name: string
  sourceProto: string
  sourcePointId: string
  sourcePointName: string
  targetProto: string
  targetTag: string
  transform: 'raw' | 'linear' | 'enum'
  scale?: number
  offset?: number
  enumMap?: { key: string; label: string }[]
  enabled: boolean
}
const form = reactive<ConvertForm>({ name: '', sourceProto: 'modbus', sourcePointId: '', sourcePointName: '', targetProto: 'mqtt', targetTag: '', transform: 'raw', scale: 1, offset: 0, enumMap: [{ key: '', label: '' }], enabled: true })

/** 历史规则可能存的是旧协议别名：打开时归一到注册表协议名，避免下拉显示空白 */
const LEGACY_PROTO_ALIAS: Record<string, string> = { 'modbus-tcp': 'modbus', 'modbus-rtu': 'modbus', 'opc-ua': 'opcua' }
const normalizeSourceProto = (p: string) => LEGACY_PROTO_ALIAS[p] ?? p

/** 源点位下拉：带出中文名便于识别；编辑历史规则时点位若已删除，仍保留原 ID 避免被清空 */
const sourcePointOptions = computed(() => {
  const opts = points.value
    .filter((p) => p.id)
    .map((p) => ({ value: p.id, label: p.name ? `${p.name}（${p.id}）` : p.id }))
    .sort((a, b) => a.label.localeCompare(b.label, 'zh-CN'))
  const cur = form.sourcePointId.trim()
  if (cur && !opts.some((o) => o.value === cur)) {
    opts.unshift({ value: cur, label: `${cur}（配置中未找到该点位）` })
  }
  return opts
})

/** 选中点位后自动带出中文名（仍可手改） */
function onSourcePointChange(id: string) {
  const hit = points.value.find((p) => p.id === id)
  if (hit) form.sourcePointName = hit.name || hit.id
}

/** 本地试跑：输入原始值后按当前表单参数即时换算（原为「数据清洗」页的浏览器内试跑能力） */
const previewRaw = ref('')
const previewResult = computed(() => {
  const raw = previewRaw.value.trim()
  if (raw === '') return '—'
  const n = Number(raw)
  if (form.transform === 'linear') {
    return Number.isFinite(n) ? String(n * (form.scale ?? 1) + (form.offset ?? 0)) : '原始值需为数值'
  }
  if (form.transform === 'enum') {
    const hit = (form.enumMap ?? []).find((m) => m.key.trim() !== '' && Number(m.key) === n)
    return hit ? hit.label || '（标签为空）' : '未匹配任何枚举项'
  }
  return raw
})

/**
 * 名称必填校验：与后端 validate_rule 的「name 不能为空」对齐，空名在提交前即拦截
 *
 * 校验口径与后端 convert.rs 的 ConvertRuleConfig::validate_rule 逐条对齐，
 * 避免用户填完表单才被后端 400 打回——那时的报错文案暴露的是后端字段名
 * （source_point_id 等），与界面上的中文标签对不上，用户无从判断该改哪一栏。
 * 后端约束：name 非空、source_point_id 非空、
 * transform=enum 时 enum_map 非空且每项 key 为有限数值、label 非空。
 * （target_tag 后端未强校验，故前端也不拦，避免拦掉后端允许的用法）
 */
const formRef = ref<FormInstance>()
/** 枚举项 key 必须是有限数值（后端按数值匹配），空串/非数字均拦截 */
function validateEnumKey(_rule: unknown, value: { key: string; label: string }[], cb: (e?: Error) => void) {
  // 仅「枚举」转换方式才要求 enum_map 合法；其余方式不提交 enum_map，不参与校验
  if (form.transform !== 'enum') return cb()
  const list = value ?? []
  if (list.length === 0) return cb(new Error('枚举映射不能为空，请至少添加一项'))
  for (const m of list) {
    if (!m.key.trim()) return cb(new Error('枚举项的值不能为空'))
    if (!Number.isFinite(Number(m.key))) return cb(new Error(`枚举项的值「${m.key}」必须是数值`))
    if (!m.label.trim()) return cb(new Error(`枚举项「${m.key}」的标签不能为空`))
  }
  cb()
}
const formRules: FormRules = {
  name: [{ required: true, message: '请输入规则名称', trigger: 'blur' }],
  sourcePointId: [{ required: true, message: '请输入或选择源点位 ID', trigger: 'change' }],
  enumMap: [{ validator: validateEnumKey, trigger: 'change' }],
}

// 弹窗未保存修改守卫：X/ESC/取消关闭前对比基线，有改动先确认
const formGuard = useDialogDirtyGuard(showForm, () => form)

function tfLabel(t: string) { return { raw: '原值', linear: '线性变换', enum: '枚举' }[t] || t }
function protoLabel(p: string): string {
  return (PROTO_LABEL as Record<string, string>)[p] || p
}

// 转换规则列定义：顺序/宽度/固定列与原 el-table 一致
const ruleColumns: ColumnDef<ConvertRule>[] = [
  { key: 'name', title: '规则名称', minWidth: 150 },
  {
    key: 'sourceProto',
    title: '源',
    width: 200,
    cell: (row) =>
      h(Fragment, null, [
        h(ElTag, { size: 'small' }, () => protoLabel(row.sourceProto)),
        h('span', { style: 'margin-left:6px' }, row.sourcePointName),
      ]),
  },
  {
    key: 'targetProto',
    title: '目标',
    width: 240,
    cell: (row) =>
      h(Fragment, null, [
        h(ElTag, { size: 'small', type: 'success' }, () => protoLabel(row.targetProto)),
        h('span', { style: 'margin-left:6px' }, row.targetTag),
      ]),
  },
  { key: 'transform', title: '转换方式', width: 100, cell: (row) => tfLabel(row.transform) },
  {
    key: 'lastValue',
    title: '当前值',
    width: 120,
    cell: (row) => (row.lastValue !== undefined ? h('span', null, row.lastValue ?? '') : h('span', null, '-')),
  },
  {
    key: 'enabled',
    title: '状态',
    width: 80,
    cell: (row) =>
      h(ElSwitch, {
        size: 'small',
        modelValue: row.enabled,
        'onUpdate:modelValue': (v: unknown) => { row.enabled = Boolean(v) },
        onChange: () => toggle(row.id),
      }),
  },
  actionColumn<ConvertRule>({
    width: 140,
    fixed: 'right',
    cell: (row) =>
      h(Fragment, null, [
        h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => edit(row) }, () => '编辑'),
        h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => remove(row) }, () => '删除'),
      ]),
  }),
]

async function load() { loading.value = true; try { rules.value = await listConvertRules() } catch (e) { showToast(e, '加载转换规则失败') } finally { loading.value = false } }
function openCreate() { Object.assign(form, { name: '', sourceProto: 'modbus', sourcePointId: '', sourcePointName: '', targetProto: 'mqtt', targetTag: '', transform: 'raw', scale: 1, offset: 0, enumMap: [{ key: '', label: '' }], enabled: true }); isEdit.value = false; showForm.value = true; if (!points.value.length) void loadPoints(); nextTick(() => formRef.value?.clearValidate()) }
function edit(row: ConvertRule) { Object.assign(form, JSON.parse(JSON.stringify(row))); form.sourceProto = normalizeSourceProto(form.sourceProto); isEdit.value = true; editingId.value = row.id; showForm.value = true; if (!points.value.length) void loadPoints() }
async function save() {
  if (!formRef.value) return
  try { await formRef.value.validate() } catch { return } // 校验失败（如名称为空）直接拦截
  const payload: Omit<ConvertRule, 'id' | 'createdAt'> = {
    name: form.name,
    sourceProto: form.sourceProto as ConvertRule['sourceProto'],
    sourcePointId: form.sourcePointId,
    sourcePointName: form.sourcePointName,
    targetProto: form.targetProto as ConvertRule['targetProto'],
    targetTag: form.targetTag,
    transform: form.transform,
    enabled: form.enabled,
  }
  if (form.transform === 'linear') { payload.scale = form.scale; payload.offset = form.offset }
  if (form.transform === 'enum') payload.enumMap = form.enumMap
  submitting.value = true
  try {
    if (isEdit.value) { await updateConvertRule(editingId.value, payload); ElMessage.success('已更新') }
    else { await createConvertRule(payload); ElMessage.success('已创建') }
    showForm.value = false; load()
  } catch (e) {
    showToast(e, '保存转换规则失败')
  } finally {
    submitting.value = false
  }
}
async function remove(row: ConvertRule) {
  if (!(await confirmDelete(row.name))) return
  try {
    await deleteConvertRule(row.id); ElMessage.success('已删除'); load()
  } catch (e) {
    showToast(e, '删除转换规则失败')
  }
}
async function toggle(id: string) {
  try {
    await toggleConvertRule(id)
  } catch (e) {
    showToast(e, '切换规则状态失败')
  }
}

onMounted(load)
</script>

<style scoped>
.pc { padding: 16px; }
.hint { margin: 4px 0 12px; font-size: 12px; color: var(--el-text-color-secondary); }
.top-bar { margin-bottom: 12px; display: flex; gap: 8px; }
.preview { display: flex; align-items: center; gap: 10px; }
.preview-arrow { color: var(--el-text-color-secondary); }
.preview-out { color: var(--el-color-primary); }
.preview-tip { margin-top: 4px; font-size: 12px; line-height: 1.5; color: var(--el-text-color-secondary); }
</style>
