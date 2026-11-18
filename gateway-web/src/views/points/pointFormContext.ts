/**
 * 单点（新增 / 编辑）表单上下文
 *
 * 背景：PointList.vue 原本把「点位清单」与「单点表单弹窗」写在同一个文件里，
 * 页面文件超过 500 行预算。拆分后表单体独立成 PointsFormDialog.vue，
 * 但表单模型（reactive 对象）、校验规则、el-form 实例引用、提交/重置回调
 * 仍然归 composable（usePoints）与页面（PointList.vue）所有——
 * 这里定义一份显式的契约，把它们交给弹窗组件使用。
 *
 * 为什么用 provide/inject 而不是 props：
 * 表单模型是 reactive 对象，模板里有二十多处 `v-model="pointForm.xxx"`。
 * 走 props 下发会触发 `vue/no-mutating-props`（改 props 的深层字段同样算改 prop），
 * 而 provide/inject 共享**同一个** reactive 对象不受该规则约束，
 * 行为与拆分前完全一致（编译产物为 `_unref(pointForm).id = $event`）。
 *
 * 关键不变量：注入方拿到的是同一个 reactive 对象与同一个 Ref，
 * 因此 `pointFormRef` 仍能接住弹窗里 `<el-form ref="pointFormRef">` 的回写，
 * composable 的 `submitPoint()` → `pointFormRef.value?.validate()` 链路不断。
 */
import type { InjectionKey, Ref } from 'vue'
import type { FormInstance, FormRules } from 'element-plus'
import type { PointForm } from '@/composables/devices/usePoints'
import type { DialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

export interface PointFormContext {
  /** 表单模型：composable 持有的 reactive 对象，弹窗直接 v-model 改字段 */
  pointForm: PointForm
  /** 校验规则（必填项 + CT/PT 变比 + 告警高低限交叉校验） */
  pointRules: FormRules
  /** el-form 实例：弹窗用 `ref="pointFormRef"` 回写，submitPoint 依赖它做校验 */
  pointFormRef: Ref<FormInstance | undefined>
  /** 新增 / 编辑 */
  formMode: Ref<'create' | 'edit'>
  /** 提交中（按钮 loading） */
  submitting: Ref<boolean>
  /** 弹窗可见性 */
  showForm: Ref<boolean>
  /** 当前设备名：总线 / 协议继承提示用 */
  currentDeviceName: Ref<string>
  /** 北向上报周期（秒）：采集周期上界。读不到配置时弹窗退回兜底值 */
  northInterval?: Ref<number>
  /** 协议中文标签表 */
  protocolLabel: Record<string, string>
  /** 未保存修改守卫（X / ESC / 取消 + 路由级脏标记桥接） */
  formGuard: DialogDirtyGuard
  /** 提交表单 */
  submitPoint: () => Promise<void>
  /** 重置表单（弹窗关闭时调用） */
  resetForm: () => void
}

/**
 * 注入键。
 *
 * 用 Symbol 而非字符串：避免与其它 provide/inject 撞名（本仓已有 hubTabLabelKey 先例）。
 */
export const pointFormContextKey: InjectionKey<PointFormContext> = Symbol('pointFormContext')
