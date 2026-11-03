/**
 * 部署向导的共享类型。
 *
 * 向导主组件（Wizard.vue）持有全部表单状态与流程编排，
 * 各步骤子组件只通过 v-model / props / emits 与主组件通信，
 * 因此这些表单模型集中在本文件，避免每个子组件各写一份形状相同的 interface。
 */

/** 向导步骤元信息：步骤条与卡片标题共用 */
export interface WizardStepMeta {
  title: string
  desc: string
}

/** 就地新建总线的表单模型（第 1 步） */
export interface NewBusForm {
  port_name: string
  baud_rate: number
  protocol: string
}

/** 设备表单模型（第 2 步）：名称 / 从站地址 / 设备模板 */
export interface DeviceFormModel {
  name: string
  slaveId: number
  template: string
}

/** 手动补录点位的表单模型（第 3 步） */
export interface ManualPointForm {
  name: string
  addressNum: number
  dataType: string
}

/** 上行转发（MQTT 主通道）表单模型（第 4 步） */
export interface UplinkFormModel {
  enabled: boolean
  broker: string
  topic: string
  publishInterval: number
  qos: number
}

/** 就地新建设备模板的表单模型（第 2 步弹出的对话框） */
export interface NewTemplateForm {
  name: string
  protocol: string
  interval: number
}

/** 单个点位的采集验证结果（第 5 步） */
export interface ReadResult {
  name: string
  id: string
  ok: boolean
  value: string
}
