<template>
  <div class="vp">
    <SectionHeader title="虚拟点位" icon="Operation" />

    <el-alert
      type="info"
      show-icon
      :closable="false"
      title="虚拟点位由网关实时求值"
      description="公式在采集流水线中按周期求值，派生量与普通点位一同上报（支持引用其它虚拟点位）。本页「公式校验」为浏览器端预演，「重算」会触发一次真实求值。"
      style="margin-bottom: 16px"
    />

    <div class="top-bar">
      <el-button type="primary" @click="openCreate"><el-icon><Plus /></el-icon> 新建虚拟点位</el-button>
      <el-button @click="load"><el-icon><Refresh /></el-icon> 刷新</el-button>
    </div>

    <DataTable
      :data="points"
      :columns="pointColumns"
      row-key="id"
      stripe
      :loading="loading"
      empty-text="暂无虚拟点位"
    />

    <!-- 新建/编辑 -->
    <el-dialog v-model="showForm" :title="isEdit ? '编辑虚拟点位' : '新建虚拟点位'" width="560px" :before-close="formGuard.beforeClose">
      <el-form label-width="90px">
        <el-form-item label="名称"><el-input v-model="form.name" placeholder="请输入虚拟点位名称" /></el-form-item>
        <el-form-item label="设备">
          <el-select v-model="form.deviceId" filterable style="width: 100%">
            <el-option v-for="d in devices" :key="d.id" :label="d.name || d.id" :value="d.id" />
          </el-select>
        </el-form-item>
        <el-form-item label="公式">
          <el-input v-model="form.formula" type="textarea" :rows="3" placeholder="{voltage} * {current} * 0.85" />
          <div class="formula-help">
            <el-tag size="small" effect="plain">支持函数：ABS / ROUND / MAX / MIN / SQRT / POW / AVG / CEIL / FLOOR / LOG / EXP / IF</el-tag>
            <el-tag size="small" effect="plain">点位引用：{point_id}</el-tag>
            <el-tag size="small" effect="plain">常量：PI / E</el-tag>
            <el-tag size="small" type="info" effect="plain">LOG 为自然对数；^ 右结合</el-tag>
          </div>
        </el-form-item>
        <el-form-item label="单位"><el-input v-model="form.unit" /></el-form-item>
        <el-form-item label="小数位"><el-input-number v-model="form.decimals" :min="0" :max="6" controls-position="right" /></el-form-item>
        <el-form-item label="状态">
          <el-switch v-model="form.enabled" active-text="启用" inactive-text="停用" />
        </el-form-item>
        <el-form-item v-if="form.formula" label="引用点位">
          <div class="refs">
            <el-tag v-for="r in refs" :key="r" size="small" effect="plain">{{ r }}</el-tag>
            <el-tag v-if="refs.length === 0" type="info" size="small">无</el-tag>
          </div>
        </el-form-item>
        <el-form-item v-if="form.formula" label="公式校验">
          <el-tag :type="checkResult.valid ? 'success' : 'danger'" size="small">
            {{ checkResult.valid ? '语法通过' : checkResult.error }}
          </el-tag>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="formGuard.close()">取消</el-button>
        <el-button type="primary" :loading="saving" :disabled="!checkResult.valid || saving" @click="save">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { ref, reactive, computed, onMounted, h } from 'vue'
import { useRoute } from 'vue-router'
import { ElButton, ElMessage, ElTag } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import {Plus, Refresh} from '@element-plus/icons-vue'
import {
  listVirtualPointsView,
  createVirtualPointView,
  updateVirtualPointView,
  deleteVirtualPoint,
  recalcVirtualPointView,
  type VirtualPointViewV,
} from '@/api/pointsOps'
import { validateFormula, extractPointRefs } from '@/utils/formulaEngine'
import { virtualPointEvalUi } from '@/utils/virtualPointStatus'
import { listDevices } from '@/api/devices'
import type { ColumnDef } from '@/components/DataTable.vue'
import { actionColumn } from '@/utils/commonColumns'

const route = useRoute()
const points = ref<VirtualPointViewV[]>([])
const devices = ref<{ id: string; name: string }[]>([])
const loading = ref(false)
const showForm = ref(false)
const isEdit = ref(false)
const editId = ref('')
/** 虚拟点位保存中：网关写接口往返期间禁用保存按钮，避免重复提交生成多个点位 */
const saving = ref(false)
const { showToast } = useErrorDisplay()

const form = reactive({ name: '', deviceId: '', formula: '', unit: '', decimals: 2, enabled: true })

const refs = computed(() => form.formula ? extractPointRefs(form.formula) : [])
const checkResult = computed(() => form.formula ? validateFormula(form.formula) : { valid: false })

// 未保存更改守卫：虚拟点位弹窗。弹窗级拦 X/ESC/取消；scope 桥接路由级注册表，
// 弹窗开着跳路由 / 切 Hub tab 仍被 router.beforeEach / HubShell 拦截。
const formGuard = useDialogDirtyGuard(showForm, () => form, { scope: route.path })

async function load() {
  loading.value = true
  try {
    points.value = await listVirtualPointsView()
    devices.value = await listDevices()
  } catch (e) {
    // 列表/设备拉取失败同样要可见，避免「空白表格」被当成没有虚拟点位
    showToast(e, '加载虚拟点位失败')
  } finally { loading.value = false }
}

function openCreate() {
  isEdit.value = false
  Object.assign(form, { name: '', deviceId: '', formula: '', unit: '', decimals: 2, enabled: true })
  showForm.value = true
}

function edit(p: VirtualPointViewV) {
  isEdit.value = true
  editId.value = p.id
  Object.assign(form, { name: p.name, deviceId: p.deviceId, formula: p.formula, unit: p.unit, decimals: p.decimals, enabled: p.enabled })
  showForm.value = true
}

async function save() {
  if (!form.name.trim()) { ElMessage.warning('请输入名称'); return }
  if (!form.deviceId) { ElMessage.warning('请选择设备'); return }
  if (!checkResult.value.valid) { ElMessage.warning('公式语法错误'); return }
  const payload = {
    name: form.name,
    device_id: form.deviceId,
    formula: form.formula,
    unit: form.unit,
    decimals: form.decimals,
    enabled: form.enabled,
  }
  saving.value = true
  try {
    if (isEdit.value) {
      try {
        await updateVirtualPointView(editId.value, payload)
        ElMessage.success('已更新')
      } catch (e) {
        // 写操作全局静默（拦截器只留痕），必须给出可见提示，否则用户以为保存成功
        showToast(e, '保存虚拟点位失败')
        return
      }
    } else {
      try {
        await createVirtualPointView(payload)
        ElMessage.success('已创建')
      } catch (e) {
        showToast(e, '新增虚拟点位失败')
        return
      }
    }
    showForm.value = false
    load()
  } finally {
    saving.value = false
  }
}

async function recalc(p: VirtualPointViewV) {
  try {
    const r = await recalcVirtualPointView(p.id)
    // 重算为真实求值：有错即报错；无错即给出计算值
    if (r.error) ElMessage.error(`重算失败: ${r.error}`)
    else if (r.value != null) ElMessage.success(`重算结果: ${r.value}`)
    else ElMessage.warning('重算未产出数值')
  } catch (e) {
    showToast(e, '重算虚拟点位失败')
  }
  load()
}

/**
 * 求值状态分类（后端把「尚未求值」也放在 `error` 字段如实上报，前端必须区分，
 * 否则刚建的点位会被标成红色「求值异常」，误导运维）：
 * 见 `utils/virtualPointStatus.ts`。
 */
async function remove(p: VirtualPointViewV) {
  if (!(await confirmDelete(p.name))) return
  try {
    await deleteVirtualPoint(p.id)
    ElMessage.success('已删除')
    load()
  } catch (e) {
    showToast(e, '删除虚拟点位失败')
  }
}

const pointColumns: ColumnDef<VirtualPointViewV>[] = [
  { key: 'name', title: '名称', width: 140 },
  {
    key: 'formula',
    title: '公式',
    minWidth: 240,
    ellipsis: true,
    cell: (row) => h('code', { class: 'formula' }, row.formula),
  },
  {
    key: 'lastValue',
    title: '当前值',
    width: 120,
    cell: (row) => {
      if (row.lastValue !== null) return h('span', { class: 'value' }, `${row.lastValue} ${row.unit}`)
      // 无值时展示引擎侧的真实原因（未求值 / 引用缺失 / 公式非法 / 已停用）
      if (row.error) return h('span', { class: 'error' }, row.error)
      return h('span', { class: 'placeholder' }, '等待采集数据')
    },
  },
  { key: 'unit', title: '单位', width: 80 },
  {
    key: 'enabled',
    title: '状态',
    width: 100,
    cell: (row) => {
      const ui = virtualPointEvalUi(row)
      return h(ElTag, { size: 'small', type: ui.type, effect: 'dark', round: true }, () => ui.text)
    },
  },
  actionColumn<VirtualPointViewV>({
    width: 200,
    fixed: 'right',
    // 操作列包一层容器：DataTable 对非 VNode 会做 String() 化
    cell: (row) =>
      h('div', null, [
        h(ElButton, { size: 'small', type: 'success', link: true, onClick: () => recalc(row) }, () => '重算'),
        h(ElButton, { size: 'small', type: 'primary', link: true, onClick: () => edit(row) }, () => '编辑'),
        h(ElButton, { size: 'small', type: 'danger', link: true, onClick: () => remove(row) }, () => '删除'),
      ]),
  }),
]

onMounted(load)
</script>

<style scoped>
.vp { padding: 0px; }
.top-bar { display: flex; gap: 8px; margin-bottom: 12px; }
.formula { font-family: monospace; background: var(--bg-fill); padding: 2px 6px; border-radius: var(--radius-small); font-size: 0.75rem; }
.value { font-weight: 600; color: var(--color-primary); }
.error { color: var(--text-danger); font-size: 0.75rem; }
.formula-help { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 6px; }
.refs { display: flex; gap: 4px; flex-wrap: wrap; }
</style>
