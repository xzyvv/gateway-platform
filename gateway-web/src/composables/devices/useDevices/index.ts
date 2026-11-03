import { onMounted } from 'vue'
import { protocolLabel, protocolType, protocolOptions, protocolAddressPlaceholder } from '../useProtocol'
import DeviceDiscoverDialog from '@/views/devices/dialogs/DeviceDiscoverDialog.vue'
import { createDeviceState } from './state'
import { createDeviceQuery } from './query'
import { createDeviceOptions } from './options'
import { createDeviceValidation } from './validation'
import { createDeviceCrud } from './crud'
import { createDeviceConnectivity } from './connectivity'

export type { DeviceFormModel, DeviceInlinePatch, DeviceSnapshot, DeviceState } from './types'

/**
 * Devices 页面数据层封装
 *
 * 把 Devices.vue 中的设备列表、增删改查、批量检测、对话框状态、路由高亮、
 * 通信质量统计全部集中到一个 composable，主组件只剩 template。
 *
 * 本文件是 barrel：具体实现按职责拆分到同目录下的
 * state（共享状态）/ query（列表加载与高亮）/ options（模板与总线选项）/
 * validation（表单校验）/ crud（增删改与表单提交）/ connectivity（连通性检测与通信质量）/
 * types（类型定义）。
 */
export function useDevices() {
  const state = createDeviceState()
  const query = createDeviceQuery(state)
  const options = createDeviceOptions(state)
  const validation = createDeviceValidation(state)
  const crud = createDeviceCrud(state, {
    loadDevices: query.loadDevices,
    loadTemplates: options.loadTemplates,
    loadBuses: options.loadBuses,
    applyTemplateProtocol: options.applyTemplateProtocol,
    isModbus: validation.isModbus,
  })
  const connectivity = createDeviceConnectivity(state, {
    isModbus: validation.isModbus,
  })

  onMounted(query.loadDevices)

  return {
    // state
    devices: state.devices, loading: state.loading, loadError: state.loadError, saving: state.saving,
    testingConn: state.testingConn, showCreate: state.showCreate, showDiscover: state.showDiscover,
    editing: state.editing, checking: state.checking,
    form: state.form, formRef: state.formRef, rules: validation.rules, formGuard: state.formGuard,
    templates: state.templates, templatesLoading: state.templatesLoading, templateLoadError: state.templateLoadError,
    buses: state.buses, busesLoading: state.busesLoading, busLoadError: state.busLoadError,
    commsStats: connectivity.commsStats,
    commsQualityType: connectivity.commsQualityType, commsQualityText: connectivity.commsQualityText,
    // protocol labels
    protocolLabel, protocolType, protocolOptions, protocolAddressPlaceholder,
    highlightId: query.highlightId, rowClass: query.rowClass,
    // actions
    loadTemplates: options.loadTemplates, loadBuses: options.loadBuses, applyTemplateProtocol: options.applyTemplateProtocol,
    loadDevices: query.loadDevices, batchCheck: connectivity.batchCheck,
    addDevice: crud.addDevice, addDeviceWithTemplate: crud.addDeviceWithTemplate, editDevice: crud.editDevice,
    resetForm: crud.resetForm, onDialogOpen: crud.onDialogOpen,
    testDeviceConn: connectivity.testDeviceConn, saveDevice: crud.saveDevice, removeDevice: crud.removeDevice,
    onDiscoverAdd: crud.onDiscoverAdd, saveDeviceField: crud.saveDeviceField,
    // component
    DeviceDiscoverDialog,
  }
}

/** useDevices 的返回值类型（供调用方标注 composable 返回值） */
export type UseDevicesReturn = ReturnType<typeof useDevices>
