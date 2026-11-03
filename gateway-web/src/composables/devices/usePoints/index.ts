import { onMounted } from 'vue'
import { protocolLabel, protocolType, protocolOptions } from '../useProtocol'
import { createPointsState } from './state'
import { createPointQuery } from './query'
import { createPointValues } from './values'
import { createPointCrud } from './crud'
import { createPointForm } from './form'
import { createPointBatchImport } from './batchImport'

export type { PointRow, PointForm, InlinePatch, BatchDeleteResult, BatchImportResult } from './types'

/**
 * Points 页面数据层封装
 *
 * 把 Points 页面中的设备/点位列表、单点 CRUD、批量删除/编辑/复制/生成对话框状态、
 * 表单状态、加载/卸载守卫全部集中到一个 composable，主组件只剩 template。
 *
 * 本文件是 barrel：具体实现按职责拆分到同目录下的
 * state（共享状态）/ query（列表加载）/ values（读值）/ crud（增删改）/
 * form（单点表单）/ batchImport（批量导入）/ validation（校验与取值收敛）/ types（类型）。
 */
export function usePoints() {
  const state = createPointsState()
  const query = createPointQuery(state)
  const values = createPointValues()
  const crud = createPointCrud(state, query.loadPoints)
  const form = createPointForm(state, query.loadPoints)
  const batchImport = createPointBatchImport(state, query.loadPoints)

  onMounted(query.loadDevices)

  return {
    // state
    devices: state.devices, points: state.points, runtimePoints: state.runtimePoints,
    mergedPoints: state.mergedPoints, loading: state.loading, loadError: state.loadError,
    selectedDevice: state.selectedDevice, currentDeviceName: state.currentDeviceName,
    selection: state.selection,
    // single point
    pointForm: form.pointForm, pointFormRef: form.pointFormRef, pointRules: form.pointRules,
    formMode: form.formMode, submitting: form.submitting, showForm: form.showForm,
    // batch
    showImport: batchImport.showImport, importText: batchImport.importText,
    importing: batchImport.importing, importProgress: batchImport.importProgress,
    showCopy: state.showCopy, showBatchEdit: state.showBatchEdit, showGen: state.showGen,
    // protocol labels
    protocolLabel, protocolType, protocolOptions,
    // actions
    onSelectionChange: query.onSelectionChange, loadDevices: query.loadDevices, loadPoints: query.loadPoints,
    readPoint: values.readPoint, removePoint: crud.removePoint, batchDelete: crud.batchDelete,
    doImport: batchImport.doImport,
    openCreate: form.openCreate, openEdit: form.openEdit, resetForm: form.resetForm,
    submitPoint: form.submitPoint, toggleWritable: crud.toggleWritable, savePointField: crud.savePointField,
  }
}

/** usePoints 的返回值类型（供调用方标注 composable 返回值） */
export type UsePointsReturn = ReturnType<typeof usePoints>
