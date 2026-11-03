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
        <el-input v-model="batchEditForm.bus" placeholder="留空则不修改">
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
import { ref, reactive, h, getCurrentInstance } from 'vue'
import { ElMessage, ElNotification, type FormInstance, type FormItemRule, type FormRules } from 'element-plus'
import { useRoute } from 'vue-router'
import { confirmAction } from '@/utils/confirm'
import { InfoFilled, Check } from '@element-plus/icons-vue'
import { updatePoint } from '@/api/points'
import type { PointConfig } from '@/api/types'
import SlaveIdInput from '@/components/industrial/SlaveIdInput.vue'
import { protocolOptions } from '@/composables/devices/useProtocol'
import { useDialogVisible } from '@/composables/core/useDialogVisible'
import { useUnmountedGuard } from '@/composables/core/useUnmountedGuard'
import { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

interface BatchEditFlags {
  name: boolean
  bus: boolean
  protocol: boolean
  slave_id: boolean
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

const batchEditSaving = ref(false)
const batchEditFlags = reactive<BatchEditFlags>({
  name: false, bus: false, protocol: false, slave_id: false,
})
const batchEditForm = reactive({
  name: '', bus: '', protocol: 'modbus', slave_id: 1,
})
const batchFormRef = ref<FormInstance>()

/** 总线只在勾选后才必填：未勾选的字段本来就不参与修改，不该拦人 */
const validateBusWhenChecked: FormItemRule['validator'] = (_rule, value, callback) => {
  if (batchEditFlags.bus && !String(value ?? '').trim()) {
    callback(new Error('已勾选总线，请填写总线名'))
    return
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
  batchEditForm.name = ''
  batchEditForm.bus = ''
  batchEditForm.protocol = 'modbus'
  batchEditForm.slave_id = 1
}

function onBatchFlagToggle(field: keyof BatchEditFlags, checked: boolean) {
  if (!checked) {
    if (field === 'name') batchEditForm.name = ''
    else if (field === 'bus') batchEditForm.bus = ''
    else if (field === 'protocol') batchEditForm.protocol = 'modbus'
    else if (field === 'slave_id') batchEditForm.slave_id = 1
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
</style>
