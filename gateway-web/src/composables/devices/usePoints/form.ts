import { reactive, ref } from 'vue'
import { ElMessage, type FormInstance } from 'element-plus'
import { createPoint, updatePoint } from '@/api/points'
import type { PointPayload } from '@/api/points'
import { failureMessage } from '@/utils/errorHandler'
import type { PointConfig } from '@/api/types'
import { clampInterval, createPointRules } from './validation'
import type { PointForm, PointsState } from './types'

/**
 * 单点新增 / 编辑表单
 *
 * 负责表单模型、回填、校验与提交；提交体与后端 `PointPayload` 对齐
 * （必填 id / bus / protocol / address / interval，且 interval 收敛到 1..=59）。
 */
export function createPointForm(state: PointsState, loadPoints: () => Promise<void>) {
  const { devices, selectedDevice } = state

  const showForm = ref(false)
  const formMode = ref<'create' | 'edit'>('create')
  const submitting = ref(false)
  const pointFormRef = ref<FormInstance>()
  const pointForm = reactive<PointForm>({
    id: '',
    name: '',
    bus: '',
    protocol: 'modbus',
    address: '',
    slave_id: 1,
    address_num: 0,
    interval: 1,
    writable: false,
    unit: '',
    scale: 1,
    offset: 0,
    formula: '',
    deadband: 0,
    on_change: false,
    alarm_high: undefined,
    alarm_low: undefined,
    aggregation_fn: '',
    aggregation_window: 60,
  })
  const pointRules = createPointRules()

  function prefillFromDevice() {
    const dev = devices.value.find((d) => d.id === selectedDevice.value)
    if (dev) {
      pointForm.bus = dev.bus
      pointForm.protocol = dev.protocol
      // Bug W4: 创建点位必须携带合法 interval（后端必填）。
      // 默认继承设备周期，但要收敛到「小于北向上报周期」，否则后端必然拒绝。
      pointForm.interval = clampInterval(dev.interval, state.northInterval.value)
    }
  }

  function openCreate() {
    if (!selectedDevice.value) return
    formMode.value = 'create'
    resetForm()
    prefillFromDevice()
    showForm.value = true
  }

  function openEdit(row: PointConfig) {
    formMode.value = 'edit'
    pointForm.id = row.id
    pointForm.name = row.name || ''
    pointForm.bus = row.bus
    pointForm.protocol = row.protocol
    pointForm.address = row.address
    pointForm.slave_id = row.slave_id ?? 1
    pointForm.address_num = row.address_num ?? 0
    // Bug W4: 编辑时保留该点位原有采集周期（后端 PUT 会无条件覆盖 interval）。
    // 这里不按北向周期设上界：历史配置可能存在大周期点位，编辑其它字段不应
    // 把用户的 300s 静默改成 59s；合法性由用户显式改周期时再收敛。
    pointForm.interval = clampInterval(row.interval)
    // S11: 回填写入白名单标记
    pointForm.writable = row.writable ?? false
    // BUG-B: 回填高级参数（编辑时不回填就会被默认值覆盖）
    pointForm.unit = row.unit ?? ''
    pointForm.scale = row.scale ?? 1
    pointForm.offset = row.offset ?? 0
    pointForm.formula = row.formula ?? ''
    pointForm.deadband = row.deadband ?? 0
    pointForm.on_change = row.on_change ?? false
    pointForm.alarm_high = row.alarm_high ?? undefined
    pointForm.alarm_low = row.alarm_low ?? undefined
    pointForm.aggregation_fn = row.aggregation?.function ?? ''
    pointForm.aggregation_window = row.aggregation?.window ?? 60
    // N3: 总线/协议继承自所属设备。历史配置里可能存着与设备不一致的值，
    // 这类点位后端建不出对应采集器、永远采不到数据，打开编辑时按设备纠正。
    prefillFromDevice()
    showForm.value = true
  }

  function resetForm() {
    pointForm.id = ''
    pointForm.name = ''
    pointForm.bus = ''
    pointForm.protocol = 'modbus'
    pointForm.address = ''
    pointForm.slave_id = 1
    pointForm.address_num = 0
    pointForm.interval = 1
    // S11: 重置写入白名单为默认拒绝
    pointForm.writable = false
    pointForm.unit = ''
    pointForm.scale = 1
    pointForm.offset = 0
    pointForm.formula = ''
    pointForm.deadband = 0
    pointForm.on_change = false
    pointForm.alarm_high = undefined
    pointForm.alarm_low = undefined
    pointForm.aggregation_fn = ''
    pointForm.aggregation_window = 60
    pointFormRef.value?.clearValidate?.()
  }

  async function submitPoint() {
    const ok = await pointFormRef.value?.validate().catch(() => false)
    if (!ok) return
    if (!selectedDevice.value) return
    submitting.value = true
    try {
      // Bug W4: 去掉 `as PointConfig` 强断言，改用与后端对齐的 PointPayload，
      // 并补齐后端必填的 interval（此前缺失 → serde 422 missing field `interval`）。
      const payload: PointPayload = {
        id: pointForm.id,
        name: pointForm.name || undefined,
        bus: pointForm.bus,
        protocol: pointForm.protocol,
        address: pointForm.address,
        slave_id: pointForm.slave_id,
        address_num: pointForm.address_num,
        interval: clampInterval(pointForm.interval, state.northInterval.value),
        device_id: selectedDevice.value,
        // S11: 随表单提交写入白名单标记
        writable: pointForm.writable,
        // BUG-B: 高级参数随请求提交（此前缺失 → 后端只收到基础字段）
        unit: pointForm.unit.trim() || undefined,
        scale: pointForm.scale,
        offset: pointForm.offset,
        formula: pointForm.formula.trim() || undefined,
        deadband: pointForm.deadband,
        on_change: pointForm.on_change,
        alarm_high: pointForm.alarm_high,
        alarm_low: pointForm.alarm_low,
        aggregation: pointForm.aggregation_fn
          ? { window: Math.max(1, Math.round(pointForm.aggregation_window)), function: pointForm.aggregation_fn }
          : undefined,
      }
      if (formMode.value === 'create') {
        await createPoint(selectedDevice.value, payload)
        if (state.isUnmounted()) return
        ElMessage.success('点位创建成功')
      } else {
        await updatePoint(pointForm.id, payload)
        if (state.isUnmounted()) return
        ElMessage.success('点位更新成功')
      }
      showForm.value = false
      loadPoints()
    } catch (e) {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示。
      // 必须带上后端原因（id 重复、配额超限、设备不存在等都是可纠正错误）
      ElMessage.error(formMode.value === 'create' ? failureMessage('创建点位', e) : failureMessage('更新点位', e))
    } finally {
      if (!state.isUnmounted()) submitting.value = false
    }
  }

  return {
    pointForm, pointFormRef, pointRules, formMode, submitting, showForm,
    openCreate, openEdit, resetForm, submitPoint,
  }
}
