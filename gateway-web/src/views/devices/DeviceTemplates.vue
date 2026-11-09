<template>
  <div class="dev-tpl">
    <SectionHeader title="设备模板" icon="Collection" />

    <div class="top-bar">
      <el-button type="primary" @click="openCreate"><el-icon><Plus /></el-icon> 新建模板</el-button>
      <el-button @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
    </div>

    <DataTable
      :data="templates"
      :columns="tplColumns"
      row-key="name"
      :loading="loading"
      stripe
    >
      <template #empty>
        <empty-state description="暂无模板，新建模板可批量复用设备点位配置" :image-size="80">
          <el-button type="primary" size="small" @click="openCreate">新建模板</el-button>
        </empty-state>
      </template>
    </DataTable>

    <!-- 新建模板对话框 -->
    <el-dialog :close-on-click-modal="false" v-model="showCreate" :title="editing ? '编辑设备模板' : '新建设备模板'" width="1080px" :before-close="createGuard.beforeClose" @close="form.name = ''; form.description = ''; form.points = []">
      <el-form label-width="100px">
        <el-form-item label="模板名称"><el-input v-model="form.name" placeholder="请输入模板名称" /></el-form-item>
        <!-- 模板是随文件分发的 JSON，说明写在这里能跟着文件走（适配固件/接线要求/变比须知） -->
        <el-form-item label="描述">
          <el-input v-model="form.description" placeholder="适配型号、固件版本、接线与变比注意事项（可留空）" />
        </el-form-item>
        <el-form-item label="协议"><ProtocolSelector v-model="form.protocol" /></el-form-item>
        <el-form-item label="默认采集周期">
          <!-- 与其他页面的周期字段统一用「数字 + 单位」，避免用户在分钟/秒之间自己换算 -->
          <DurationSelect v-model="form.defaultInterval" :min="1" :max="3600" :units="['s', 'min']" unit-width="82px" />
          <span class="tpl-hint">设备未单独配置采集周期时使用该值</span>
        </el-form-item>
        <el-form-item label="点位列表">
          <div class="point-list">
            <div v-for="(p, i) in form.points" :key="i" class="point-row">
              <el-input v-model="p.name" placeholder="名称" style="width: 120px" />
              <el-input v-model="p.address" placeholder="0 基偏移" style="width: 96px" />
              <!-- 功能码决定读哪个区（保持/输入/线圈/离散），也决定列表展示地址的区段 -->
              <el-select v-model="p.function_code" style="width: 128px">
                <el-option v-for="fc in FUNCTION_CODES" :key="fc.value" :label="fc.label" :value="fc.value" />
              </el-select>
              <el-select v-model="p.data_type" style="width: 92px">
                <el-option v-for="t in DATA_TYPES" :key="t" :label="t" :value="t" />
              </el-select>
              <el-input v-model="p.unit" placeholder="单位" style="width: 72px" />
              <!-- 变比留空=不启用；电流/功率/电量吃 CT，电压/功率/电量吃 PT -->
              <el-input-number v-model="p.ct_ratio" :min="0" :controls="false" placeholder="CT" style="width: 84px" />
              <el-input-number v-model="p.pt_ratio" :min="0" :controls="false" placeholder="PT" style="width: 84px" />
              <el-button size="small" type="danger" link @click="form.points.splice(i, 1)">删除</el-button>
            </div>
            <el-button size="small" @click="addPoint"><el-icon><Plus /></el-icon> 新增点位</el-button>
          </div>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createGuard.close">取消</el-button>
        <el-button type="primary" :loading="creating" :disabled="creating" @click="save">{{ editing ? '保存' : '创建' }}</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, onMounted, h } from 'vue'
import { useRouter } from 'vue-router'
import { ElButton, ElMessage } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import {Plus, Refresh} from '@element-plus/icons-vue'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'
import {
  listTemplates, saveTemplate, deleteTemplate,
  type DeviceTemplate, type TemplatePoint,
} from '@/api/templates'
import ProtocolSelector from '@/components/industrial/ProtocolSelector.vue'
import DurationSelect from '@/components/industrial/DurationSelect.vue'
import { DATA_TYPES } from '@/composables/devices/decodeParams'
import { formatDateTime } from '@/utils/datetime'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'

/** 表单点位类型：在 API TemplatePoint 基础上补充表单所需字段 */
interface FormPoint extends TemplatePoint {
  /** 表单编辑的是 0 基寄存器偏移（后端 default_address），不是 5 位展示地址 */
  address: string
  function_code: number
  unit: string
  data_type: string
  ct_ratio: number | null
  pt_ratio: number | null
}

/** Modbus 功能码：区段决定读请求类型，也决定列表展示地址的首位 */
const FUNCTION_CODES = [
  { value: 3, label: '03 保持寄存器' },
  { value: 4, label: '04 输入寄存器' },
  { value: 1, label: '01 线圈' },
  { value: 2, label: '02 离散输入' },
]

const templates = ref<DeviceTemplate[]>([])
const loading = ref(false)
const router = useRouter()
const showCreate = ref(false)
/** 编辑态：与「点位模板」页合并后，同一份 DeviceTemplate 实体在此页完成增/改/删 */
const editing = ref(false)
/** 模板创建中：网关写接口往返期间禁用按钮，避免重复提交生成多个模板 */
const creating = ref(false)
const { showToast } = useErrorDisplay()

function emptyPoint(): FormPoint {
  return {
    name: '',
    address: '0',
    function_code: 3,
    data_type: 'u16',
    unit: '',
    ct_ratio: null,
    pt_ratio: null,
  }
}

function emptyForm() {
  return {
    name: '', description: '', protocol: 'modbus',
    /** 模板默认采集周期（秒）：设备未显式配置 interval 时按此值展开 */
    defaultInterval: 1,
    points: [emptyPoint()] as FormPoint[],
  }
}

const form = reactive(emptyForm())

// 新建设备模板弹窗未保存修改守卫：X/ESC/取消时若有改动先确认（source 含 points 数组，JSON 序列化对比）
const createGuard = useDialogDirtyGuard(showCreate, () => form)

// 模板表统一走 DataTable：列顺序/宽度与迁移前保持一致
const tplColumns: ColumnDef<DeviceTemplate>[] = [
  { key: 'name', title: '模板名称', minWidth: 160 },
  { key: 'description', title: '描述', minWidth: 200, ellipsis: true },
  { key: 'protocol', title: '协议', width: 100 },
  { key: 'points', title: '点位数', width: 90, cell: (row) => row.points.length },
  {
    key: 'createdAt',
    title: '创建时间',
    width: 170,
    cell: (row) =>
      row.createdAt
        ? formatDateTime(row.createdAt as number)
        : '-',
  },
  actionColumn<DeviceTemplate>({
    width: 220,
    fixed: 'right',
    cell: (row) => [
      h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => openEdit(row) }, () => '编辑'),
      h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => applyTemplate(row) }, () => '用此模板新建设备'),
      h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => remove(row) }, () => '删除'),
    ],
  }),
]

function addPoint() {
  // 地址默认接着上一行递增，避免新增点位还要自己数偏移
  const next = emptyPoint()
  next.address = String(form.points.length)
  form.points.push(next)
}

async function load() {
  loading.value = true
  try { templates.value = await listTemplates() } catch (e) { showToast(e, '加载设备模板失败') } finally { loading.value = false }
}

function openCreate() {
  editing.value = false
  Object.assign(form, emptyForm())
  showCreate.value = true
}

/** 编辑既有模板：后端 /api/templates 以 name 为键覆盖，故保存即更新 */
function openEdit(t: DeviceTemplate) {
  editing.value = true
  form.name = t.name
  form.description = String(t.description ?? '')
  form.protocol = t.protocol || 'modbus'
  // 默认采集周期：后端字段为 default_interval，历史模板可能缺失 → 兜底 1 秒
  form.defaultInterval = Number(t.default_interval ?? 1)
  // 用展开运算保留表单未编辑的字段（unit/scale/offset/formula/deadband…），
  // 否则「编辑模板再保存」会把这些字段整体抹掉，等于静默破坏模板
  form.points = (Array.isArray(t.points) ? t.points : []).map((p) => ({
    ...p,
    name: String(p.name ?? ''),
    address: String(p.default_address ?? p.address ?? ''),
    function_code: Number(p.function_code ?? 3),
    data_type: String(p.data_type ?? 'u16'),
    unit: String(p.unit ?? ''),
    ct_ratio: p.ct_ratio == null ? null : Number(p.ct_ratio),
    pt_ratio: p.pt_ratio == null ? null : Number(p.pt_ratio),
  }))
  if (form.points.length === 0) form.points = [emptyPoint()]
  showCreate.value = true
}

async function save() {
  if (!form.name.trim()) { ElMessage.warning('请输入模板名称'); return }
  if (form.points.some((p) => !p.name.trim())) { ElMessage.warning('请填写所有点位名称'); return }
  // 变比必须 > 0：0 会把一次侧值乘成 0，负数会反号；留空表示不启用
  for (const p of form.points) {
    const invalid = (p.ct_ratio != null && !(Number(p.ct_ratio) > 0))
      || (p.pt_ratio != null && !(Number(p.pt_ratio) > 0))
    if (invalid) {
      ElMessage.warning(`${p.name.trim() || '点位'} 的 CT/PT 变比必须大于 0（无需变比请留空）`)
      return
    }
  }
  creating.value = true
  try {
    // F-01：后端 DeviceTemplate 的契约是 protocol / default_interval 必填，
    // 点位字段名为 default_address（不是 address）、data_id（不是 register_type）。
    // 旧提交体既缺 default_interval，又用错点位字段名，另外还缺 vendor/model，
    // 结果整份反序列化失败——「新建模板」实际不可用。
    const payload: DeviceTemplate = {
      name: form.name.trim(),
      vendor: '',
      model: '',
      // Option<String>：留空就不写，避免把空串当成有效描述
      description: form.description.trim() || undefined,
      protocol: form.protocol,
      default_interval: form.defaultInterval,
      points: form.points.map((p) => {
        const addr = p.address.trim() === '' ? undefined : Number(p.address)
        return {
          // 展开保留表单未编辑的字段（unit/scale/offset/register_count/function_code…），
          // 否则编辑模板再保存会把它们整体抹掉
          ...p,
          name: p.name.trim(),
          address: p.address,
          default_address: addr,
          data_type: p.data_type || undefined,
          ct_ratio: p.ct_ratio ?? undefined,
          pt_ratio: p.pt_ratio ?? undefined,
        }
      }),
    }
    await saveTemplate(payload)
    ElMessage.success(editing.value ? '已保存' : '已创建')
    showCreate.value = false
    Object.assign(form, emptyForm())
    load()
  } catch (e) {
    // 写操作全局静默（拦截器只留痕），必须给出可见提示，否则用户以为保存成功
    showToast(e, editing.value ? '保存设备模板失败' : '创建设备模板失败')
  } finally {
    creating.value = false
  }
}

async function remove(t: DeviceTemplate) {
  if (!(await confirmDelete(t.name))) return
  try {
    await deleteTemplate(t.name)
    ElMessage.success('已删除')
    load()
  } catch (e) {
    showToast(e, '删除设备模板失败')
  }
}

function applyTemplate(t: DeviceTemplate) {
  // S7: 原名「应用到新设备」只弹一条 toast，什么都不做，名不副实。
  // 现改为真正跳到设备页并带出预填参数：新增设备弹窗自动打开且模板已选好。
  // tab 必须显式指回「设备清单」：设备模板本身是 DeviceHub 的一个页签，
  // 只改 query 不切 tab 的话 DeviceList 不会挂载，弹窗自然打不开（实测踩到）。
  void router.push({
    path: '/devices',
    query: { tab: 'devices', action: 'create', template: t.name },
  })
}

onMounted(load)
</script>

<style scoped>
.dev-tpl { padding: 0px; }
.top-bar { display: flex; gap: 8px; margin-bottom: 12px; }
.point-list { display: flex; flex-direction: column; gap: 6px; width: 100%; }
.point-row { display: flex; gap: 6px; align-items: center; }
.tpl-hint { margin-left: 10px; color: var(--text-secondary); font-size: 0.75rem; }
</style>
