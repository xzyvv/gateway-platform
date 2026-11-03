/**
 * 表单字段级错误高亮工具。
 *
 * 全站约 7 处跨字段校验（起始/结束地址、时间范围、路径前缀等）用 ElMessage 弹窗
 * 提示，但不给对应的 el-form-item 加 `.is-error` 态，用户不知道该改哪个字段。
 *
 * 本工具通过 Element Plus `FormInstance.getField(prop)` 拿到 FormItem 的 context
 * （含 reactive 的 validateState / validateMessage），直接写入 error 态，
 * 并顺手把错误与控件做 ARIA 关联（aria-invalid + aria-describedby）——
 * EP 2.14 只在错误节点上渲染 role="alert"，控件侧没有任何关联，读屏器
 * 聚焦到输入框时听不到错误内容。
 * 不 patch ElMessage，不引入额外依赖。
 *
 * 职责边界：EP 原生 rules 校验的错误不经本工具，其 ARIA 关联不在此职责内。
 *
 * 用法：
 *   const formRef = ref<FormInstance>()
 *   // 校验失败时：
 *   setFormFieldErrors(formRef!, { start: '不能大于结束地址', end: '不能小于起始地址' })
 *   // 校验通过 / 提交前：
 *   clearFormFieldErrors(formRef!, ['start', 'end'])
 */
import { nextTick } from 'vue'
import type { FormInstance } from 'element-plus'

export type FieldErrors = Record<string, string>

/**
 * 给 form 的指定字段设置错误态（让 el-form-item 显示红色边框 + 错误文字）。
 * 通过 FormInstance.getField(prop) 获取 FormItem context，直接写 validateState / validateMessage。
 *
 * 关键：EP FormItem 的 context 是 reactive({ ...toRefs(props), validateState, validateMessage, ... })。
 * Vue reactive 会自动解包内部的 ref——所以 field.validateState 是 string（不是 Ref<string>），
 * 必须直接赋值 field.validateState = 'error'，不能写 .value。
 *
 * 用 validator API（form.validateField + rules 里加错误 rule）也能做到，但走 reactive 写值
 * 更轻：不触发 rule 重校验，也不依赖 form 有没有 :model。
 *
 * @param formRef el-form 实例（模板里 `ref="formRef"`）
 * @param errors 字段 prop → 错误消息的映射；空字符串视为清除该字段错误
 */
export function setFormFieldErrors(formRef: FormInstance, errors: FieldErrors): void {
  const getField = (formRef as unknown as { getField?: (prop: string) => FormItemContext | undefined })
    .getField
  if (typeof getField !== 'function') return
  for (const [prop, msg] of Object.entries(errors)) {
    const field = getField(prop)
    if (!field) continue
    if (msg) {
      // reactive 自动解包 ref，直接赋值
      field.validateState = 'error'
      field.validateMessage = msg
      wireErrorAria(field)
    } else {
      field.clearValidate()
      clearErrorAria(field)
    }
  }
}

/**
 * 清除指定字段的错误态。不传 props 则清除全部。
 *
 * 已知局限：不传 props（清全部）时 EP Form 未暴露字段列表，无法逐字段摘除
 * aria-invalid / aria-describedby；残留属性会在下次 setFormFieldErrors 覆盖或页面
 * 切换时复位。需要彻底清理的场景请带 props 调用（本项目用法均带 props）。
 */
export function clearFormFieldErrors(formRef: FormInstance, props?: string[]): void {
  if (props) {
    formRef.clearValidate(props)
    // EP 的 clearValidate 只清校验状态，不管理 ARIA——逐字段摘掉关联属性
    const getField = (formRef as unknown as { getField?: (prop: string) => FormItemContext | undefined })
      .getField
    if (typeof getField !== 'function') return
    for (const prop of props) {
      const field = getField(prop)
      if (field) clearErrorAria(field)
    }
  } else {
    formRef.clearValidate()
  }
}

/**
 * EP FormItem context（从 form.getField 返回）的最小必要接口。
 * 注意：validateState / validateMessage 在 reactive 里会自动解包 ref，是 string 不是 Ref。
 */
interface FormItemContext {
  validateState: string
  validateMessage: string
  clearValidate: () => void
  /** form-item 根元素（context 内的 formItemRef 经 reactive 解包后即元素本身） */
  $el?: HTMLElement | null
  /** EP 为 label 生成的唯一 id，用作错误消息 id 的命名空间 */
  labelId?: string
}

/** 错误消息节点的稳定 id：派生自 EP labelId，同一 form-item 内唯一且可复现。 */
function errorIdOf(field: FormItemContext): string | undefined {
  return field.labelId ? `${field.labelId}-error` : undefined
}

/** 找到 form-item 内真正承载输入的控件：原生 input/textarea 优先，
 *  没有则回退选择器包裹器（el-select 的 .el-select__wrapper）或 combobox 角色。 */
function fieldControl(root: HTMLElement): HTMLElement | null {
  return root.querySelector<HTMLElement>('input:not([type=hidden]), textarea')
    ?? root.querySelector<HTMLElement>('.el-select__wrapper, [role="combobox"]')
}

/** 写入 ARIA 关联：控件 aria-invalid="true"，错误消息节点挂 id 并被 aria-describedby 引用。
 *  错误节点在 validateMessage 写入后才由 EP 的 v-if 渲染出来，等 nextTick 再挂 id；
 *  回调先复查状态——若期间字段已被 clear，不能把属性加回去（set→clear 同步连发的竞态）。 */
function wireErrorAria(field: FormItemContext): void {
  void nextTick(() => {
    if (field.validateState !== 'error') return
    const root = field.$el
    if (!root) return
    const control = fieldControl(root)
    if (!control) return
    control.setAttribute('aria-invalid', 'true')
    const id = errorIdOf(field)
    if (!id) return
    const msg = root.querySelector<HTMLElement>('.el-form-item__error')
    if (msg) {
      msg.id = id
      control.setAttribute('aria-describedby', id)
    }
  })
}

/** 摘除单字段的 ARIA 错误关联。移除属性无需等渲染，同步执行。 */
function clearErrorAria(field: FormItemContext): void {
  const root = field.$el
  if (!root) return
  const control = fieldControl(root)
  control?.removeAttribute('aria-invalid')
  control?.removeAttribute('aria-describedby')
}
