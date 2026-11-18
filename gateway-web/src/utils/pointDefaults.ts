import type { PointConfig } from '@/api/types'

/**
 * 点位默认值工厂。
 *
 # 为什么要收口

新增点位有四条路径——批量导入 CSV、模板展开、批量生成、向导手工添加——每条路径
都要手写一遍「22 个可选字段各填什么」。历史上有 4 份几乎一样的字面量：

 * `views/devices/batchImport/pointCsv.ts`   `defaultPointConfig()`
 * `composables/devices/usePoints/state.ts`  模板展开的最小 PointConfig
 * `views/points/dialogs/PointsGenerateDialog.vue`  批量生成
 * `views/system/config/wizard/usePointStep.ts`      向导手工添加

它们本该是同一份语义，却各自漂移：模板展开那处多一个 `writable: false`（导入的点位
默认不写入现场寄存器），向导那处干脆不传 `writable`。改一个默认值要改四个地方，
漏一处就出现「同一个字段在不同入口默认不同」这种难查的问题。

 # 契约

`defaultPointConfig()` 返回一份**新对象**（含全新数组/对象字段），调用方可以安全
地就地修改，不会污染其他调用方。

`writable` 的默认值是 `false`：批量导入、模板展开、批量生成三条路径都是「先把点位
建出来，人工确认后再决定能不能写」。向导路径历史上不传该字段（由后端取默认），
已改为显式传 `false`——两处行为对齐，且后端对缺省的处理本就等价于 false。
 */

export type PointDefaultOverrides = Partial<PointConfig>

/** 默认点位配置：返回独立副本，调用方可就地修改 */
export function defaultPointConfig(overrides: PointDefaultOverrides = {}): PointConfig {
  return {
    id: '',
    name: null,
    device_id: null,
    bus: '',
    protocol: '',
    address: '',
    slave_id: null,
    address_num: null,
    data_id: null,
    function_code: null,
    data_type: null,
    byte_order: null,
    register_count: null,
    interval: 1,
    scale: 1,
    offset: 0,
    formula: null,
    ct_ratio: null,
    pt_ratio: null,
    deadband: 0,
    on_change: false,
    unit: null,
    alarm_high: null,
    alarm_low: null,
    aggregation: null,
    modbus_register: null,
    // S11：批量导入 / 模板展开的点位默认不在写入白名单内（避免误写现场寄存器）
    writable: false,
    ...overrides,
  }
}
