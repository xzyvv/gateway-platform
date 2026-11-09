<template>
  <el-dialog v-model="visible" title="批量修改点位" width="560px" @close="resetBatchEditForm" :before-close="batchFormGuard.beforeClose">
    <div class="batch-tip">
      <el-icon><InfoFilled /></el-icon>
      <span>
        已选 <strong>{{ selection.length }}</strong> 个点位。仅勾选并填写下方字段会应用到所选点位，未勾选字段保持原值。
      </span>
    </div>
    <el-form ref="batchFormRef" :model="batchEditForm" :rules="batchRules" label-width="90px" scroll-to-error>
      <el-form-item label="名称" prop="name">
        <el-input v-model="batchEditForm.name" placeholder="留空则不修改">
          <template #prepend>
            <el-checkbox v-model="batchEditFlags.name" @change="onBatchFlagToggle('name', $event)" />
          </template>
        </el-input>
      </el-form-item>
      <el-form-item label="总线" prop="bus">
        <!-- 总线名必须与运行参数里的总线完全一致：手填错一个字符，点位就永远采不到数据 -->
        <el-select
          v-if="buses.length"
          v-model="batchEditForm.bus"
          filterable
          :loading="busesLoading"
          :disabled="!batchEditFlags.bus"
          placeholder="选择总线"
          style="width: 100%"
        >
          <template #prefix>
            <el-checkbox v-model="batchEditFlags.bus" @change="onBatchFlagToggle('bus', $event)" />
          </template>
          <el-option v-for="b in buses" :key="b.port_name" :label="b.port_name" :value="b.port_name">
            <span>{{ b.port_name }}</span>
            <span class="bus-opt-meta">{{ protocolLabel[b.protocol] || b.protocol }}</span>
          </el-option>
        </el-select>
        <!-- 总线列表加载失败时退回文本输入，避免弹窗直接不可用 -->
        <el-input v-else v-model="batchEditForm.bus" placeholder="留空则不修改">
          <template #prepend>
            <el-checkbox v-model="batchEditFlags.bus" @change="onBatchFlagToggle('bus', $event)" />
          </template>
        </el-input>
      </el-form-item>
      <el-form-item label="协议" prop="protocol">
        <el-select v-model="batchEditForm.protocol" placeholder="不修改" :disabled="!batchEditFlags.protocol" style="width: 100%">
          <template #prefix>
            <el-checkbox v-model="batchEditFlags.protocol" @change="onBatchFlagToggle('protocol', $event)" />
          </template>
          <el-option v-for="o in protocolOptions" :key="o.value" :label="o.label" :value="o.value" />
        </el-select>
      </el-form-item>
      <el-form-item label="从站号" prop="slave_id">
        <SlaveIdInput v-model="batchEditForm.slave_id" :disabled="!batchEditFlags.slave_id" />
        <el-checkbox v-model="batchEditFlags.slave_id" @change="onBatchFlagToggle('slave_id', $event)" class="batch-check-side" />
      </el-form-item>
      <!-- 变比：勾选后填写即覆盖，勾选但留空表示清空该变比（>0 才有效） -->
      <el-form-item label="CT 变比" prop="ct_ratio">
        <el-input-number
          v-model="batchEditForm.ct_ratio"
          :min="0.001"
          :step="0.1"
          :precision="4"
          :disabled="!batchEditFlags.ct_ratio"
          controls-position="right"
          placeholder="留空=清空该变比"
          style="width: 100%"
        />
        <el-checkbox v-model="batchEditFlags.ct_ratio" @change="onBatchFlagToggle('ct_ratio', $event)" class="batch-check-side" />
      </el-form-item>
      <el-form-item label="PT 变比" prop="pt_ratio">
        <el-input-number
          v-model="batchEditForm.pt_ratio"
          :min="0.001"
          :step="0.1"
          :precision="4"
          :disabled="!batchEditFlags.pt_ratio"
          controls-position="right"
          placeholder="留空=清空该变比"
          style="width: 100%"
        />
        <el-checkbox v-model="batchEditFlags.pt_ratio" @change="onBatchFlagToggle('pt_ratio', $event)" class="batch-check-side" />
      </el-form-item>
      <!-- 解码参数：多寄存器类型（f32/u32/u64）必须「类型 + 字节序 + 寄存器数」配套，
           后端 PUT 会无条件覆盖这三个字段，逐条改既慢又容易漏 -->
      <el-form-item label="数据类型" prop="data_type">
        <el-select
          v-model="batchEditForm.data_type"
          :disabled="!batchEditFlags.data_type"
          clearable
          placeholder="不修改（留空=清空）"
          style="width: 100%"
        >
          <template #prefix>
            <el-checkbox v-model="batchEditFlags.data_type" @change="onBatchFlagToggle('data_type', $event)" />
          </template>
          <el-option v-for="t in DATA_TYPES" :key="t" :label="t" :value="t" />
        </el-select>
      </el-form-item>
      <el-form-item label="字节序" prop="byte_order">
        <el-select
          v-model="batchEditForm.byte_order"
          :disabled="!batchEditFlags.byte_order"
          clearable
          placeholder="不修改（留空=清空）"
          style="width: 100%"
        >
          <template #prefix>
            <el-checkbox v-model="batchEditFlags.byte_order" @change="onBatchFlagToggle('byte_order', $event)" />
          </template>
          <el-option v-for="o in BYTE_ORDERS" :key="o.value" :label="o.label" :value="o.value" />
        </el-select>
      </el-form-item>
      <el-form-item label="寄存器数" prop="register_count">
        <el-input-number
          v-model="batchEditForm.register_count"
          :min="1"
          :max="125"
          :disabled="!batchEditFlags.register_count"
          controls-position="right"
          placeholder="留空=按数据类型自动"
          style="width: 100%"
        />
        <el-checkbox
          v-model="batchEditFlags.register_count"
          @change="onBatchFlagToggle('register_count', $event)"
          class="batch-check-side"
        />
      </el-form-item>
    </el-form>
    <template #footer>
      <el-button @click="batchFormGuard.close">取消</el-button>
      <el-button type="primary" :loading="batchEditSaving" :disabled="batchEditSaving" @click="submitBatchEdit">
        <el-icon><Check /></el-icon> 应用到 {{ selection.length }} 个点位
      </el-button>
    </template>
  </el-dialog>
</template>

<script setup lang="ts">
import { ref, reactive, h, getCurrentInstance, watch } from 'vue'
import { ElMessage, ElNotification, type FormInstance, type FormItemRule, type FormRules } from 'element-plus'
import { useRoute } from 'vue-router'
import { confirmAction } from '@/utils/confirm'
import { InfoFilled, Check } from '@element-plus/icons-vue'
import { updatePoint } from '@/api/points'
import { getConfig } from '@/api/config'
import type { BusConfig, PointConfig } from '@/api/types'
import SlaveIdInput from '@/components/industrial/SlaveIdInput.vue'
import { protocolLabel, protocolOptions } from '@/composables/devices/useProtocol'
import { useDialogVisible } from '@/composables/core/useDialogVisible'
import { useUnmountedGuard } from '@/composables/core/useUnmountedGuard'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'
import { BYTE_ORDERS, DATA_TYPES } from '@/composables/devices/decodeParams'

interface BatchEditFlags {
  name: boolean
  bus: boolean
  protocol: boolean
  slave_id: boolean
  ct_ratio: boolean
  pt_ratio: boolean
  data_type: boolean
  byte_order: boolean
  register_count: boolean
}

const props = defineProps<{
  modelValue: boolean
  selection: PointConfig[]
  selectedDevice: string
}>()
const emit = defineEmits<{
  'update:modelValue': [value: boolean]
  edited: []
}>()

const { isUnmounted } = useUnmountedGuard()

const visible = useDialogVisible(props, emit)

/** 可选总线：与设备表单同源（/api/config 的 buses）；加载失败退回文本输入 */
const buses = ref<BusConfig[]>([])
const busesLoading = ref(false)
async function loadBuses() {
  busesLoading.value = true
  try {
    const cfg = await getConfig()
    buses.value = cfg.buses || []
  } catch {
    // 读不到总线列表不影响批量改其他字段，静默退回文本框
    buses.value = []
  } finally {
    busesLoading.value = false
  }
}
watch(visible, (open) => {
  if (open && buses.value.length === 0) void loadBuses()
}, { immediate: true })

const batchEditSaving = ref(false)
const batchEditFlags = reactive<BatchEditFlags>({
  name: false, bus: false, protocol: false, slave_id: false,
  ct_ratio: false, pt_ratio: false,
  data_type: false, byte_order: false, register_count: false,
})
const batchEditForm = reactive<{
  name: string
  bus: string
  protocol: string
  slave_id: number
  ct_ratio: number | null
  pt_ratio: number | null
  data_type: string
  byte_order: string
  register_count: number | null
}>({
  name: '', bus: '', protocol: 'modbus', slave_id: 1, ct_ratio: null, pt_ratio: null,
  data_type: '', byte_order: '', register_count: null,
})
const batchFormRef = ref<FormInstance>()

/** 总线只在勾选后才必填：未勾选的字段本来就不参与修改，不该拦人 */
const validateBusWhenChecked: FormItemRule['validator'] = (_rule, value, callback) => {
  if (batchEditFlags.bus && !String(value ?? '').trim()) {
    callback(new Error('已勾选总线，请选择总线'))
    return
  }
  callback()
}

/**
 * 寄存器数校验：勾选后留空表示「按数据类型自动推断」，填了必须在 1-125。
 * 超过 125 会超出单个 Modbus 读请求上限（后端按 125 拆分，但多半是填错了）。
 */
const validateRegisterCountWhenChecked: FormItemRule['validator'] = (_rule, value, callback) => {
  if (!batchEditFlags.register_count || value == null) return callback()
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 1 || value > 125) {
    return callback(new Error('寄存器数需为 1-125 的整数（勾选后留空表示自动）'))
  }
  callback()
}

// 字段级校验规则：错误落在对应输入框下方，而不是提交后才弹一条 toast
const batchRules: FormRules = {
  name: [
    { max: 64, message: '名称不能超过 64 个字符', trigger: 'blur' },
  ],
  bus: [
    { validator: validateBusWhenChecked, trigger: 'blur' },
  ],
  protocol: [
    { required: true, message: '请选择协议', trigger: 'change' },
  ],
  slave_id: [
    { type: 'number', min: 0, max: 247, message: '从站号需在 0-247 之间（0 为广播地址）', trigger: 'change' },
  ],
  ct_ratio: [
    { validator: validateRatioWhenChecked('CT'), trigger: 'change' },
  ],
  pt_ratio: [
    { validator: validateRatioWhenChecked('PT'), trigger: 'change' },
  ],
  register_count: [
    { validator: validateRegisterCountWhenChecked, trigger: 'change' },
  ],
}

/**
 * 变比校验：只有勾选该字段时才生效。
 *
 * 勾选后留空表示清空变比（合法的「取消放大」操作），填了数字就必须 > 0，
 * 否则 0 会把一次侧值乘成 0。
 */
function validateRatioWhenChecked(label: string): FormItemRule['validator'] {
  return (_rule, value, callback) => {
    const field = label === 'CT' ? 'ct_ratio' : 'pt_ratio'
    if (!batchEditFlags[field]) return callback()
    if (value == null) return callback()
    if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
      return callback(new Error(`${label} 变比必须大于 0（勾选后留空表示清空）`))
    }
    callback()
  }
}

// 弹窗未保存修改守卫：X/ESC/取消关闭前对比基线（含字段勾选标记），有改动先确认
// 路由级脏标记桥接：只有挂了 router 的运行环境才取路由，取不到就只启用弹窗级守卫
const dirtyScope = getCurrentInstance()?.appContext.config.globalProperties.$router ? useRoute().path : undefined
const batchFormGuard = useDialogDirtyGuard(visible, () => ({ batchEditForm, batchEditFlags }), dirtyScope ? { scope: dirtyScope } : {})

function resetBatchEditForm() {
  batchEditFlags.name = false
  batchEditFlags.bus = false
  batchEditFlags.protocol = false
  batchEditFlags.slave_id = false
  batchEditFlags.ct_ratio = false
  batchEditFlags.pt_ratio = false
  batchEditFlags.data_type = false
  batchEditFlags.byte_order = false
  batchEditFlags.register_count = false
  batchEditForm.name = ''
  batchEditForm.bus = ''
  batchEditForm.protocol = 'modbus'
  batchEditForm.slave_id = 1
  batchEditForm.ct_ratio = null
  batchEditForm.pt_ratio = null
  batchEditForm.data_type = ''
  batchEditForm.byte_order = ''
  batchEditForm.register_count = null
}

function onBatchFlagToggle(field: keyof BatchEditFlags, checked: boolean) {
  if (!checked) {
    if (field === 'name') batchEditForm.name = ''
    else if (field === 'bus') batchEditForm.bus = ''
    else if (field === 'protocol') batchEditForm.protocol = 'modbus'
    else if (field === 'slave_id') batchEditForm.slave_id = 1
    else if (field === 'ct_ratio') batchEditForm.ct_ratio = null
    else if (field === 'pt_ratio') batchEditForm.pt_ratio = null
    else if (field === 'data_type') batchEditForm.data_type = ''
    else if (field === 'byte_order') batchEditForm.byte_order = ''
    else if (field === 'register_count') batchEditForm.register_count = null
  }
}

/** 单条失败的明细：id + 名称 + 原因，供用户定位该重试哪一条 */
interface BatchEditFailure {
  id: string
  name: string | null
  reason: string
}

/** 逐条并发的结果聚合（禁止只回笼统的成功/失败计数） */
interface BatchEditResult {
  total: number
  succeeded: number
  failed: BatchEditFailure[]
}

/** 并发上限：只改所选点位（几条到几十条），逐条 PUT 以拿到逐条结果 */
const CONCURRENCY = 8

/** 逐条调用 updatePoint（worker pool），单条失败不阻断其余点位 */
async function applyUpdatesOneByOne(updates: PointConfig[]): Promise<BatchEditResult> {
  const total = updates.length
  const queue = [...updates]
  let succeeded = 0
  const failed: BatchEditFailure[] = []
  async function worker() {
    while (!isUnmounted()) {
      const p = queue.shift()
      if (!p) break
      try {
        await updatePoint(p.id, p)
        succeeded++
      } catch (e) {
        failed.push({ id: p.id, name: p.name, reason: e instanceof Error ? e.message : String(e || '未知错误') })
      }
    }
  }
  await Promise.all(Array.from({ length: Math.min(CONCURRENCY, total) }, () => worker()))
  return { total, succeeded, failed }
}

/**
 * 按聚合结果如实提示：全成功 / 部分失败 / 全失败三种情况不能共用一句话。
 * 部分失败用常驻通知列出明细，而不是 3 秒后消失的计数。
 */
function reportBatchEditResult(result: BatchEditResult, activeFields: string[]) {
  if (result.failed.length === 0) {
    ElMessage.success('已修改 ' + result.succeeded + ' 个点位')
    return
  }
  if (result.succeeded === 0) {
    ElMessage.error('批量修改失败：' + result.failed[0]!.reason)
    return
  }
  ElMessage.success('已修改 ' + result.succeeded + '/' + result.total + ' 个点位')
  ElNotification({
    type: 'warning',
    title: '批量修改：' + result.failed.length + ' 个点位未成功',
    duration: 0,
    message: h('div', [
      h('p', { style: 'margin:0 0 6px;font-size: 0.75rem;color:var(--text-secondary)' },
        '成功 ' + result.succeeded + ' 个，失败 ' + result.failed.length + ' 个（字段: ' + activeFields.join(', ') + '）。以下点位需重试或逐条核对：'),
      ...result.failed.slice(0, 8).map((f) =>
        h('div', { style: 'font-size: 0.75rem;line-height:1.7;word-break:break-all' },
          f.id + (f.name ? '/' + f.name : '') + ' — ' + f.reason)),
      result.failed.length > 8
        ? h('div', { style: 'font-size: 0.75rem;color:var(--text-secondary);margin-top:4px' },
          '其余 ' + (result.failed.length - 8) + ' 条略，可在日志中查看')
        : null,
    ]),
  })
}

async function submitBatchEdit() {
  if (batchEditSaving.value) return
  if (!props.selectedDevice || props.selection.length === 0) return
  const activeFields = (Object.keys(batchEditFlags) as Array<keyof BatchEditFlags>).filter((k) => batchEditFlags[k])
  if (activeFields.length === 0) {
    ElMessage.warning('请勾选至少一个要修改的字段')
    return
  }
  // 字段级校验：错误已落在对应输入框下方（scroll-to-error 自动定位首个错误项），不通过就不提交
  const formEl = batchFormRef.value
  if (formEl) {
    const ok = await formEl.validate().catch(() => false)
    if (!ok) return
  }
  if (!(await confirmAction({
    message: '确认对 ' + props.selection.length + ' 个点位执行批量修改?未勾选的字段将保持原值。',
    title: '批量修改',
    confirmText: '确认修改',
  }))) return
  if (isUnmounted()) return
  batchEditSaving.value = true
  try {
    const updates: PointConfig[] = props.selection.map((p) => {
      const merged: PointConfig = { ...p }
      if (batchEditFlags.name) merged.name = batchEditForm.name || null
      if (batchEditFlags.bus) merged.bus = batchEditForm.bus
      if (batchEditFlags.protocol) merged.protocol = batchEditForm.protocol
      if (batchEditFlags.slave_id) merged.slave_id = batchEditForm.slave_id
      if (batchEditFlags.ct_ratio) merged.ct_ratio = batchEditForm.ct_ratio
      if (batchEditFlags.pt_ratio) merged.pt_ratio = batchEditForm.pt_ratio
      // 留空 = 清空（写 null），与单点表单的「默认/清空」语义一致
      if (batchEditFlags.data_type) merged.data_type = batchEditForm.data_type || null
      if (batchEditFlags.byte_order) merged.byte_order = batchEditForm.byte_order || null
      if (batchEditFlags.register_count) merged.register_count = batchEditForm.register_count
      return merged
    })
    const result = await applyUpdatesOneByOne(updates)
    if (isUnmounted()) return
    reportBatchEditResult(result, activeFields)
    if (result.succeeded > 0) {
      visible.value = false
      resetBatchEditForm()
      emit('edited')
    }
  } catch {
    // 逐条模式下不会出现「全部未改动」，此分支仅兜底未预期的中断
    ElMessage.error('批量修改请求异常中断，已成功修改的点位会保留，请刷新列表核对')
  } finally {
    if (!isUnmounted()) batchEditSaving.value = false
  }
}
</script>

<style scoped>
.batch-tip { display: flex; align-items: center; gap: 6px; padding: 8px 12px; background: var(--color-primary-bg); border-radius: var(--radius-small); margin-bottom: 16px; color: var(--color-primary); }
.batch-check-side { margin-left: 8px; }
.bus-opt-meta { margin-left: 8px; color: var(--text-secondary); font-size: 0.75rem; }
</style>
