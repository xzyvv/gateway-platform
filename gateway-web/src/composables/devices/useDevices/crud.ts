/**
 * 设备增删改与表单提交
 *
 * 含新增 / 编辑弹窗的打开与回填、表单提交、行内编辑保存、
 * 删除与「8 秒内可撤销」的删除流程。
 * 写操作失败全局不再弹窗（拦截器只留痕），因此本模块必须给出带动作语境的可见提示。
 */
import { h, ref } from 'vue'
import { ElMessage, ElNotification, ElButton } from 'element-plus'
import { confirmDelete } from '@/utils/confirm'
import { createDevice, updateDevice, deleteDevice } from '@/api/devices'
import { failureMessage } from '@/utils/errorHandler'
import type { DeviceConfig } from '@/api/types'
import type { DeviceCrudDeps, DeviceInlinePatch, DeviceSnapshot, DeviceState } from './types'

/** 删除后的撤销窗口：确认框提示与通知停留时长共用同一个值，避免两处文案对不上 */
const UNDO_WINDOW_MS = 8000

/**
 * 设备增删改
 *
 * @param state 共享状态（form / showCreate / devices / audit / 卸载守卫）
 * @param deps  跨模块依赖（列表刷新、表单选项、协议判定），由 index.ts 注入避免循环引用
 */
export function createDeviceCrud(state: DeviceState, deps: DeviceCrudDeps) {
  const { form, formRef, editing, showCreate, devices } = state

  /** S7: 从「设备模板」页跳转过来时待预填的模板名。
   *  弹窗打开时 onDialogOpen 会 resetForm（防止新建继承上一台设备的字段），
   *  所以预填必须等到 reset 之后再回填，否则会被清成空串。 */
  const pendingTemplate = ref('')

  function addDevice() {
    resetForm()
    showCreate.value = true
  }

  /** S7: 带指定模板打开新增设备弹窗（供「用此模板新建设备」跳转使用） */
  function addDeviceWithTemplate(template: string) {
    pendingTemplate.value = template
    addDevice()
  }

  function editDevice(row: DeviceConfig) {
    resetForm()
    editing.value = true
    Object.assign(form, row)
    showCreate.value = true
  }

  function resetForm() {
    editing.value = false
    // Bug W2: 需重置设备表单全部字段，否则新建时会继承上一台设备的
    // slave_id / address_map / interval（导致采集打到错误从站/寄存器）。
    Object.assign(form, {
      id: '',
      name: '',
      template: '',
      bus: '',
      protocol: 'modbus',
      address: '',
      slave_id: null,
      address_map: null,
      interval: null,
      group: '',
      tags: [],
    })
    formRef.value?.clearValidate()
  }

  function onDialogOpen() {
    if (!editing.value) {
      resetForm()
      if (pendingTemplate.value) {
        form.template = pendingTemplate.value
        pendingTemplate.value = ''
      }
    }
    // S3: 弹窗打开时刷新模板列表（模板可能在「设备模板」页新建/删除过）
    // N1: 同时刷新总线列表（总线可能在「运行参数」页增删过）
    void Promise.all([deps.loadTemplates(), deps.loadBuses()]).then(() => {
      // N2: 模板列表就绪后按模板回填协议（编辑旧设备时也顺带纠正不一致的历史配置）
      if (form.template) deps.applyTemplateProtocol(form.template)
    })
  }

  /** 设备发现弹窗选中一条扫描结果 → 回填表单并打开新增弹窗 */
  function onDiscoverAdd(row: Record<string, unknown>, bus: string, protocol: string) {
    const addr = String(row.address ?? '')
    form.bus = bus
    form.protocol = (protocol || 'modbus') as DeviceConfig['protocol']
    form.name = String(row.device_id || `${bus}_${addr}`)
    showCreate.value = true
  }

  /**
   * 由设备名派生可读且唯一的设备 ID。
   *
   * S2 修复：原实现 `form.name.trim().replace(/[^\w-]/g, '_')` 里的 `\w` 只含
   * ASCII 字母数字，中文名会被整段替换成下划线（「电表1」→「__1」），
   * 不同中文名（「电表1」「水表1」以外的任意中文名）极易撞成同一个 ID；
   * 后端当时也不校验 ID 唯一性，于是两条设备同 ID 并存，
   * 而删除按 id 全量匹配 → 删一台会连带删掉另一台及其全部点位。
   *
   * 现在：保留中文/字母/数字（`\p{L}\p{N}`），空白与符号折叠为单个下划线；
   * 若派生结果与现有设备 ID 冲突，追加短后缀直到唯一（后端仍会兜底校验）。
   */
  function deriveDeviceId(name: string): string {
    const cleaned = name
      .trim()
      .replace(/[^\p{L}\p{N}_-]/gu, '_')
      .replace(/_{2,}/g, '_')
      .replace(/^_+|_+$/g, '')
    const base = cleaned || 'dev'
    const taken = new Set(devices.value.map((d) => d.id))
    if (!taken.has(base)) return base
    for (let i = 2; i < 1000; i += 1) {
      const candidate = `${base}_${i}`
      if (!taken.has(candidate)) return candidate
    }
    return `${base}_${Date.now()}`
  }

  async function saveDevice() {
    if (!formRef.value) return
    try {
      await formRef.value.validate()
    } catch {
      return
    }
    if (state.isUnmounted()) return
    state.saving.value = true
    // 归一化下发字段：
    // - interval：0/null 一律下发 null（后端 Option<u64> 的 None = 使用模板默认周期），
    //   避免 0 被当作「覆盖为 0 秒」导致展开出的点位 interval=0 校验失败；
    // - slave_id：仅 Modbus 有意义，其他协议置 null，避免残留值误导。
    const payload = {
      ...form,
      interval: form.interval ? form.interval : null,
      slave_id: deps.isModbus() ? (form.slave_id ?? null) : null,
    } as unknown as DeviceConfig
    try {
      if (editing.value) {
        await updateDevice(form.id, payload)
        if (state.isUnmounted()) return
        ElMessage.success('更新成功')
      } else {
        form.id = deriveDeviceId(form.name)
        payload.id = form.id
        await createDevice(payload)
        if (state.isUnmounted()) return
        ElMessage.success('创建成功')
      }
      showCreate.value = false
      resetForm()
      deps.loadDevices()
    } catch (e) {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示。
      // 必须带上后端原因：只写「请稍后重试」会让配额超限/无写权限这类可纠正错误无从下手
      ElMessage.error(failureMessage(editing.value ? '更新设备' : '创建设备', e))
    } finally {
      if (!state.isUnmounted()) state.saving.value = false
    }
  }

  /**
   * 行内编辑保存：复用整行配置，仅覆盖被改的字段。
   *
   * PUT 是整行覆盖，所以必须只下发 DeviceConfig 声明的字段——列表接口可能带有
   * group/tags/enabled 等扩展字段，原样回传会被当成未知配置项。
   * 成功后只回写本地行，不重载列表（否则页码复位、正在编辑的位置也会丢）。
   */
  async function saveDeviceField(row: DeviceConfig, patch: DeviceInlinePatch): Promise<boolean> {
    if (state.isUnmounted()) return false
    const field = Object.keys(patch)[0] ?? ''
    const fieldName = field === 'name' ? '设备名' : field
    const payload: DeviceConfig = {
      id: row.id,
      name: row.name,
      template: row.template,
      bus: row.bus,
      protocol: row.protocol,
      address: row.address,
      slave_id: row.slave_id ?? null,
      address_map: row.address_map ?? null,
      interval: row.interval || 0,
      ...patch,
    }
    try {
      await updateDevice(row.id, payload)
      if (state.isUnmounted()) return false
      Object.assign(row, patch)
      ElMessage.success(`设备 ${row.id} 的${fieldName}已保存`)
      return true
    } catch {
      // 写操作失败全局静默（仅留痕），这里必须给出可见提示，否则用户以为改成功了
      ElMessage.error(`设备 ${row.id} 的${fieldName}保存失败，已保留原值`)
      return false
    }
  }

  /** 撤销删除：用删除前的完整配置快照重新创建设备（后端为硬删除，无恢复接口） */
  async function undoDelete(s: DeviceSnapshot) {
    try {
      await createDevice(s as unknown as DeviceConfig)
      ElMessage.success(`设备 ${s.id} 已恢复`)
      deps.loadDevices()
    } catch {
      // 最常见原因是同 ID 设已被重建，createDevice 唯一性校验失败
      ElMessage.error(`恢复设备 ${s.id} 失败（同 ID 设备可能已重建），请手动处理`)
    }
  }

  async function removeDevice(row: DeviceConfig) {
    if (!(await confirmDelete(row.name, { undoSeconds: UNDO_WINDOW_MS / 1000 }))) return
    if (state.isUnmounted()) return
    try {
      await deleteDevice(row.id)
      if (state.isUnmounted()) return
      ElMessage.success('删除成功')
      // 撤销窗口：通知停留 8s，期间可一键恢复；误删高发场景（选错行）无需重填整张表单
      const snapshot: DeviceSnapshot = {
        id: row.id,
        name: row.name,
        template: row.template,
        bus: row.bus,
        protocol: row.protocol,
        address: row.address,
        slave_id: row.slave_id ?? null,
        address_map: row.address_map ?? null,
        interval: row.interval ? row.interval : null,
      }
      ElNotification({
        title: '设备已删除',
        message: h('div', { style: 'display:flex;align-items:center;gap:8px' }, [
          h('span', null, `${row.name} (${row.id})`),
          h(
            ElButton,
            { size: 'small', type: 'primary', link: true, onClick: () => void undoDelete(snapshot) },
            () => '撤销删除',
          ),
        ]),
        duration: UNDO_WINDOW_MS,
      })
      deps.loadDevices()
    } catch (e) {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error(failureMessage('删除设备', e))
    }
  }

  return {
    addDevice, addDeviceWithTemplate, editDevice, resetForm, onDialogOpen, onDiscoverAdd,
    saveDevice, saveDeviceField, removeDevice,
  }
}
