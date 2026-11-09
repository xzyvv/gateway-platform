import { reactive, ref, watch } from 'vue'
import { ElMessage, type FormInstance, type FormRules } from 'element-plus'
import { createPoint, updatePoint } from '@/api/points'
import type { PointPayload } from '@/api/points'
import { failureMessage } from '@/utils/errorHandler'
import type { PointConfig } from '@/api/types'
import { ratioScopeOf } from '@/composables/devices/useRatio'
import { modbusAddressOf } from './modbusAddress'
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
    ct_ratio: null,
    pt_ratio: null,
    function_code: 3,
    data_type: '',
    byte_order: '',
    register_count: null,
    formula: '',
    deadband: 0,
    on_change: false,
    alarm_high: undefined,
    alarm_low: undefined,
    aggregation_fn: '',
    aggregation_window: 60,
  })
  const pointRules: FormRules = {
    ...createPointRules(),
    /**
     * CT/PT 变比必须大于 0（留空表示不放大）。
     *
     * 只校验当前单位下真正生效的一侧：另一侧在单位切换时已被置空，
     * 若仍拦截会让「电压点位残留的 CT 值」卡住提交。
     */
    ct_ratio: [
      {
        trigger: 'change',
        validator: (_rule, value, callback) => {
          const scope = ratioScopeOf(pointForm.unit)
          if (scope !== 'ct' && scope !== 'both') return callback()
          if (value == null) return callback()
          if (!Number.isFinite(Number(value)) || Number(value) <= 0) {
            return callback(new Error('CT 变比必须大于 0（无需变比请留空）'))
          }
          callback()
        },
      },
    ],
    pt_ratio: [
      {
        trigger: 'change',
        validator: (_rule, value, callback) => {
          const scope = ratioScopeOf(pointForm.unit)
          if (scope !== 'pt' && scope !== 'both') return callback()
          if (value == null) return callback()
          if (!Number.isFinite(Number(value)) || Number(value) <= 0) {
            return callback(new Error('PT 变比必须大于 0（无需变比请留空）'))
          }
          callback()
        },
      },
    ],
    /**
     * 告警高低限交叉校验：低限必须小于高限。
     * 任意一侧留空表示该侧不启用，不做限制；两侧都填时才比较。
     */
    alarm_high: [
      {
        trigger: 'change',
        validator: (_rule, _value, callback) => {
          const high = pointForm.alarm_high
          const low = pointForm.alarm_low
          if (high != null && low != null && low >= high) {
            callback(new Error('低限必须小于高限'))
            return
          }
          callback()
        },
      },
    ],
  }

  /**
   * 单位变化时清空不生效的一侧变比。
   *
   * 变比是否生效由单位决定（电压只吃 PT、电流只吃 CT、功率因数两者都不吃），
   * 留着已失效的数字会让用户误以为仍在参与计算；置空后提交也不会写进配置。
   */
  watch(
    () => pointForm.unit,
    (unit) => {
      const scope = ratioScopeOf(unit)
      if (scope === 'pt' || scope === 'none') pointForm.ct_ratio = null
      if (scope === 'ct' || scope === 'none') pointForm.pt_ratio = null
    },
  )

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

  /**
   * Modbus 下把 `address` 统一成「区段 + 偏移」推导出的 5 位展示串。
   *
   * 真实寻址用的是 `function_code + address_num`，`address` 只做展示/去重；
   * 两者不一致时按实际寻址值纠正展示串，避免表单显示 40001 却读了 30011。
   * 非 Modbus 协议（DLT645 表地址等）保持原样。
   */
  function syncModbusAddress() {
    if (pointForm.protocol !== 'modbus') return
    const derived = modbusAddressOf(pointForm.function_code, pointForm.address_num)
    if (derived) pointForm.address = derived
  }

  function openCreate() {
    if (!selectedDevice.value) return
    formMode.value = 'create'
    resetForm()
    prefillFromDevice()
    syncModbusAddress()
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
    // 变比与解码参数：后端 PUT 会无条件覆盖这些字段，不回填 = 编辑即清空
    pointForm.ct_ratio = row.ct_ratio ?? null
    pointForm.pt_ratio = row.pt_ratio ?? null
    // 不把 null 改写成 3：非 Modbus 协议的功能码另有语义（DLT645 控制码），
    // 原样回传才不会被「打开编辑点保存」悄悄写进一个 3。
    pointForm.function_code = row.function_code ?? null
    pointForm.data_type = row.data_type ?? ''
    pointForm.byte_order = row.byte_order ?? ''
    pointForm.register_count = row.register_count ?? null
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
    // Modbus 展示地址与 function_code/address_num 对齐（见 syncModbusAddress）
    syncModbusAddress()
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
    pointForm.ct_ratio = null
    pointForm.pt_ratio = null
    pointForm.function_code = 3
    pointForm.data_type = ''
    pointForm.byte_order = ''
    pointForm.register_count = null
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
      // 变比按单位决定是否生效：不生效的一侧提交 null，避免后端长期存着
      // 一个永不参与计算、改单位后却突然放大的变比。
      const ratioScope = ratioScopeOf(pointForm.unit)
      const ratioOrNull = (v: number | null) =>
        v == null || !Number.isFinite(v) ? null : v
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
        ct_ratio: ratioScope === 'ct' || ratioScope === 'both' ? ratioOrNull(pointForm.ct_ratio) : null,
        pt_ratio: ratioScope === 'pt' || ratioScope === 'both' ? ratioOrNull(pointForm.pt_ratio) : null,
        // 寻址/解码参数：后端 PUT 无条件覆盖，必须整行回传否则编辑即丢失
        function_code: pointForm.function_code ?? undefined,
        data_type: pointForm.data_type.trim() || undefined,
        byte_order: pointForm.byte_order.trim() || undefined,
        register_count: pointForm.register_count ?? undefined,
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
