import type { ComputedRef, Ref } from 'vue'
import type { AggFn, DeviceConfig, PointConfig } from '@/api/types'
import type { PointQuality } from '@/api/dataQuality'

/**
 * 点位清单行：配置点位（可编辑）+ 模板展开的运行时点位（只读，_source='template'）。
 *
 * 设备用模板配置时点位由 scheduler 启动展开、不落 config.points，
 * 配置 API 查不到——清单只看配置会出现「总览 4 个点位、清单 0 个」的割裂。
 * 模板点位只读展示（可读取当前值），不可编辑/删除/勾选：管理动作请改模板。
 */
export type PointRow = PointConfig & { _source?: 'template' }

/**
 * 单点新增 / 编辑表单模型
 *
 * 字段与后端 `PointPayload` 对齐（id / bus / protocol / address / interval 必填）。
 */
export interface PointForm {
  id: string
  name: string
  bus: string
  protocol: string
  address: string
  slave_id: number
  address_num: number
  /** Bug W4: 后端 PointPayload 必填，需随表单一起提交。 */
  interval: number
  /** S11: 写入白名单标记（false=拒绝写入，默认） */
  writable: boolean
  // BUG-B: 高级参数。此前表单不收集这些字段，后端 `PointPayload` 又忽略未知字段，
  // 用户在别处填的告警阈值/死区/聚合会静默丢失。现在表单直接编辑并随请求提交。
  unit: string
  scale: number
  offset: number
  /**
   * CT / PT 变比（null=不放大）。
   *
   * 后端 `convert_value` 会按单位决定是否参与计算，表单侧也只提交生效项，
   * 避免给电压点位误存 CT 变比后在下次改单位时突然放大 30 倍。
   */
  ct_ratio: number | null
  pt_ratio: number | null
  /**
   * Modbus 功能码：与 address_num 一起构成真正的寻址方式。
   * 非 Modbus 协议另有语义（DLT645/CJT188 用作控制码），编辑时必须原样回传。
   */
  function_code: number | null
  /** 数值解码类型（u16 / float32 …），仅 Modbus 等统一数值解码协议使用 */
  data_type: string
  /** 字节序 ABCD/DCBA/CDAB/BADC，留空走后端默认 ABCD */
  byte_order: string
  /** 占用寄存器数，留空按数据类型自动推断 */
  register_count: number | null
  formula: string
  deadband: number
  on_change: boolean
  alarm_high: number | undefined
  alarm_low: number | undefined
  /** 聚合函数；空串=不聚合 */
  aggregation_fn: '' | AggFn
  /** 聚合窗口（秒），仅 aggregation_fn 非空时生效 */
  aggregation_window: number
}

/** 行内编辑允许改的字段：只挑不会改变采集语义的展示/寻址信息 */
export type InlinePatch = Partial<Pick<PointConfig, 'name' | 'address' | 'slave_id'>>

/** 批量操作中单条失败的明细（点位 id + 失败原因） */
export interface PointFailure {
  id: string
  reason: string
}

/** 批量删除结果：成功数 + 失败明细 */
export interface BatchDeleteResult {
  deleted: number
  failed: PointFailure[]
}

/** 批量导入结果：成功数 + 失败明细 */
export interface BatchImportResult {
  created: number
  failed: PointFailure[]
}

/**
 * 点位页共享状态
 *
 * 加载（query）/ 增删改（crud）/ 读写值（values）/ 表单（form）/ 批量导入（batchImport）
 * 各模块共用的响应式状态与卸载守卫，统一由 state.ts 创建后注入，
 * 避免各模块各自维护一份导致状态割裂。
 */
export interface PointsState {
  /** 设备列表 */
  devices: Ref<DeviceConfig[]>
  /** 当前设备的配置点位 */
  points: Ref<PointConfig[]>
  /** 运行时点位快照（含模板展开）：进页随设备列表一起拉，失败不阻塞配置点位展示 */
  runtimePoints: Ref<PointQuality[]>
  /** 列表加载中 */
  loading: Ref<boolean>
  /** 加载失败原因；空串表示无错误。失败时不显示空表，避免误判「设备/点位被删」 */
  loadError: Ref<string>
  /** 当前选中的设备 id */
  selectedDevice: Ref<string>
  /** 表格多选 */
  selection: Ref<PointConfig[]>
  /**
   * 北向上报周期（秒）：点位采集周期必须小于它。
   *
   * 进页时从 `/api/config` 读取 mqtt.publish_interval；MQTT 未启用或读取失败时
   * 退回 FALLBACK_NORTH_INTERVAL（60），仅用于表单提示与新建点位的取值收敛。
   */
  northInterval: Ref<number>
  /** 当前设备名（未选中时为 —） */
  currentDeviceName: ComputedRef<string>
  /** 展示用点位 = 配置点位 + 模板展开的运行时点位 */
  mergedPoints: ComputedRef<PointRow[]>
  /** 复制到其他设备对话框可见性 */
  showCopy: Ref<boolean>
  /** 批量修改对话框可见性 */
  showBatchEdit: Ref<boolean>
  /** 批量生成对话框可见性 */
  showGen: Ref<boolean>
  /** 组件是否已卸载（卸载后丢弃异步结果） */
  isUnmounted: () => boolean
}
