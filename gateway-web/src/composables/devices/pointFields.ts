import { getPointFields } from '@/api/acquisition/pointFields'
import { applyByteOrders, applyDataTypes } from './decodeParams'
import { applyUnits } from './useUnits'

/** 是否已成功拉取过（失败不置位，下次进页面会重试） */
let loaded = false

/**
 * 拉取点位字段字典，刷新「数据类型 / 字节序 / 单位」三张下拉。
 *
 * 三张表的取值集以本接口为准，前端只保留兜底常量。失败（离线调试、旧后端无
 * 该接口）时静默保留兜底，不打断页面——下拉仍可用，只是可能缺后端新增的取值。
 */
export async function loadPointFields(force = false): Promise<void> {
  if (loaded && !force) return
  try {
    const dict = await getPointFields()
    applyDataTypes(dict.data_types ?? [])
    applyByteOrders(dict.byte_orders ?? [])
    applyUnits(dict.units ?? [])
    loaded = true
  } catch {
    // 保留兜底清单
  }
}
