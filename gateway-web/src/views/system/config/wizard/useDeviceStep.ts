/**
 * 部署向导 · 第 2 步（新增设备）的状态与真实接口调用。
 *
 * 设备创建是真实写入：模板必选，成功后拿到设备 id，后续点位步依赖它。
 * 这里持有设备与模板相关的全部响应式状态，页面只把状态透传给子组件渲染。
 *
 * 典型应用场景：部署向导的第 2 步（新增设备）与「就地新建模板」对话框。
 */
import { ref } from 'vue'
import { ElMessage } from 'element-plus'
import { createDevice } from '@/api/devices'
import { listTemplates, saveTemplate, type DeviceTemplate } from '@/api/templates'
import type { DeviceConfig } from '@/api/types'
import type { DeviceFormModel, NewTemplateForm } from './types'

export function useDeviceStep() {
  /** 网关模板库 */
  const templates = ref<DeviceTemplate[]>([])
  /** 设备表单值（name / slaveId / template） */
  const deviceForm = ref<DeviceFormModel>({ name: '', slaveId: 1, template: '' })
  /** 已创建设备（点位步依赖它的 id 与地址） */
  const createdDevice = ref<DeviceConfig | null>(null)

  /** 是否展示「新建模板」对话框 */
  const showNewTemplate = ref(false)
  /** 新建模板请求进行中 */
  const savingTemplate = ref(false)
  /** 新建模板的表单值 */
  const newTemplate = ref<NewTemplateForm>({ name: '', protocol: 'modbus', interval: 5 })

  /** 读取模板库；拿不到不阻断，可在第 2 步就地新建模板 */
  async function loadTemplates() {
    try {
      templates.value = await listTemplates()
    } catch {
      // 模板列表拿不到不阻断：可在第 2 步就地新建模板
      templates.value = []
    }
  }

  /** 生成设备 id：总线名 + 从站地址归一化后的组合 */
  function makeDeviceId(portName: string, address: string): string {
    const portSlug = portName.replace(/[^a-zA-Z0-9]+/g, '_').replace(/^_+|_+$/g, '')
    const addrSlug = address.replace(/[^a-zA-Z0-9]+/g, '_')
    return `${portSlug || 'bus'}_${addrSlug || 'dev'}`
  }

  /** 建设备；成功返回 true */
  async function createDev(bus: { port_name: string; protocol: string } | undefined): Promise<boolean> {
    if (!bus) {
      ElMessage.warning('请先选择总线')
      return false
    }
    const name = deviceForm.value.name.trim()
    if (!name) {
      ElMessage.warning('请填写设备名称')
      return false
    }
    if (!deviceForm.value.template) {
      ElMessage.warning('请选择设备模板：网关要求每台设备绑定模板')
      return false
    }
    try {
      const address = String(deviceForm.value.slaveId)
      const payload: DeviceConfig = {
        id: makeDeviceId(bus.port_name, address),
        name,
        template: deviceForm.value.template,
        bus: bus.port_name,
        protocol: bus.protocol,
        address,
        slave_id: deviceForm.value.slaveId,
        address_map: null,
        interval: 1,
      }
      createdDevice.value = await createDevice(payload)
      if (!createdDevice.value?.id) createdDevice.value = payload
      ElMessage.success(`已创建设备 ${name}`)
      return true
    } catch (e) {
      ElMessage.error(e instanceof Error && e.message ? e.message : '创建设备失败')
      return false
    }
  }

  /** 打开「新建模板」对话框并清空表单 */
  function openNewTemplate() {
    newTemplate.value = { name: '', protocol: 'modbus', interval: 5 }
    showNewTemplate.value = true
  }

  /** 新建设备模板；成功后自动选中该模板 */
  async function saveNewTemplate() {
    const name = newTemplate.value.name.trim()
    if (!name) {
      ElMessage.warning('请填写模板名称')
      return
    }
    if (templates.value.some((t) => t.name === name)) {
      ElMessage.warning(`模板「${name}」已存在，直接在上方选择它即可`)
      return
    }
    savingTemplate.value = true
    try {
      await saveTemplate({
        name,
        // F-01：显式提交 vendor/model（后端已加 serde(default) 兜底，但显式更直观）
        vendor: '',
        model: '',
        protocol: newTemplate.value.protocol,
        default_interval: newTemplate.value.interval,
        points: [],
      })
      await loadTemplates()
      deviceForm.value.template = name
      showNewTemplate.value = false
      ElMessage.success(`已创建模板「${name}」并自动选用，点位可到「点位管理」补充`)
    } catch (e) {
      ElMessage.error(e instanceof Error && e.message ? e.message : '创建模板失败')
    } finally {
      savingTemplate.value = false
    }
  }

  return {
    templates,
    deviceForm,
    createdDevice,
    showNewTemplate,
    savingTemplate,
    newTemplate,
    loadTemplates,
    createDev,
    openNewTemplate,
    saveNewTemplate,
  }
}
