/**
 * useDevices 目录内的公共类型定义
 *
 * 只放类型，不含运行时逻辑：设备表单模型、行内编辑补丁、删除撤销快照、
 * 以及各职责模块共用的共享状态接口。
 * 这些类型原先内联在 useDevices.ts 里，拆分后集中一处，
 * 便于各子模块（列表加载 / 表单校验 / 增删改 / 连通性）共享同一份定义。
 */
import type { Ref } from 'vue'
import type { FormInstance } from 'element-plus'
import type { useRoute } from 'vue-router'
import type { DeviceTemplate } from '@/api/templates'
import type { BusConfig, DeviceConfig } from '@/api/types'
import type { useDialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

/**
 * 设备表单模型：DeviceConfig 全部字段 + 扩展字段 group/tags
 *
 * 与后端 DeviceConfig 的差异：
 * - slave_id / interval 允许 null（留空 = 使用模板默认值）
 * - address_map 允许 null
 * - 多出 group / tags 两个前端分组字段
 */
export interface DeviceFormModel {
  id: string
  name: string
  template: string
  bus: string
  protocol: DeviceConfig['protocol']
  address: string
  slave_id: number | null
  address_map: { [key: string]: number } | null
  interval: number | null
  group: string
  tags: string[]
}

/**
 * 行内编辑补丁：仅允许覆盖 DeviceConfig 声明的字段
 *
 * PUT 是整行覆盖，所以行内编辑只能改 DeviceConfig 里真实存在的字段，
 * 不能把列表接口带来的 group/tags/enabled 等扩展字段塞进去。
 */
export type DeviceInlinePatch = Partial<Pick<DeviceConfig, 'name'>>

/**
 * 删除撤销快照：仅含 DeviceConfig 声明字段（与 saveDeviceField 的下发口径一致），
 * 列表接口附带的 group/tags 等扩展字段不进快照——重新创建时原样回传会被后端拒绝。
 */
export interface DeviceSnapshot {
  id: string
  name: string
  template: string
  bus: string
  protocol: DeviceConfig['protocol']
  address: string
  slave_id: number | null
  address_map: DeviceConfig['address_map']
  interval: number | null
}

/**
 * 设备页共享状态
 *
 * 列表加载（query）/ 表单校验（validation）/ 增删改（crud）/ 连通性（connectivity）
 * 各模块共用的响应式状态与卸载守卫，统一由 state.ts 创建后注入，
 * 避免各模块各自维护一份导致状态割裂。
 */
export interface DeviceState {
  /** 设备列表 */
  devices: Ref<DeviceConfig[]>
  /** 列表加载中 */
  loading: Ref<boolean>
  /** 列表加载失败原因；空串表示无错误。失败时不能只显示「暂无设备」，否则会误判设备被删 */
  loadError: Ref<string>
  /** 设备保存中（新建 / 编辑） */
  saving: Ref<boolean>
  /** 单设备连通性探测中 */
  testingConn: Ref<boolean>
  /** 批量连通性检测中 */
  checking: Ref<boolean>
  /** 新建 / 编辑弹窗可见性 */
  showCreate: Ref<boolean>
  /** 设备发现弹窗可见性 */
  showDiscover: Ref<boolean>
  /** 当前表单处于「编辑已有设备」模式 */
  editing: Ref<boolean>
  /** el-form 实例引用（校验入口） */
  formRef: Ref<FormInstance | undefined>
  /** 设备表单模型 */
  form: DeviceFormModel
  /** 可选设备模板列表 */
  templates: Ref<DeviceTemplate[]>
  templatesLoading: Ref<boolean>
  templateLoadError: Ref<string>
  /** 可选总线列表 */
  buses: Ref<BusConfig[]>
  busesLoading: Ref<boolean>
  busLoadError: Ref<string>
  /** 当前路由（query.id / query.highlight 用于高亮定位） */
  route: ReturnType<typeof useRoute>
  /** 设备表单弹窗的未保存更改守卫 */
  formGuard: ReturnType<typeof useDialogDirtyGuard>
  /** 组件是否已卸载（卸载后丢弃异步结果、不再写状态） */
  isUnmounted: () => boolean
}

/**
 * 增删改模块的外部依赖
 *
 * 增删改要刷新列表、刷新表单选项并按协议归一化下发字段，
 * 这些能力分属 query / options / validation 三个模块，这里显式注入避免模块间循环引用。
 */
export interface DeviceCrudDeps {
  /** 重新拉取设备列表（保存 / 删除后） */
  loadDevices: () => Promise<void>
  /** 刷新设备模板列表 */
  loadTemplates: () => Promise<void>
  /** 刷新总线列表 */
  loadBuses: () => Promise<void>
  /** 按模板带出协议 */
  applyTemplateProtocol: (name: string) => void
  /** 当前表单是否为 Modbus 协议 */
  isModbus: () => boolean
}

/**
 * 连通性模块的外部依赖
 *
 * 表单内「测试连接」需要知道当前协议（决定从站号语义），由 validation 模块注入。
 */
export interface DeviceConnectivityDeps {
  /** 当前表单是否为 Modbus 协议 */
  isModbus: () => boolean
}
