import { describe, it, expect, vi } from 'vitest'
import { nextTick } from 'vue'
import type { Mock } from 'vitest'
import { setFormFieldErrors, clearFormFieldErrors } from './formFieldError'

/** 单个表单项的最小桩：validateState / validateMessage / clearValidate + ARIA 所需的 $el / labelId */
interface FieldStub {
  validateState: string
  validateMessage: string
  clearValidate: Mock<() => void>
  $el: HTMLElement
  labelId: string
}

// 用例只碰 start / end 两个字段，按字面量联合声明可省掉每处 `!` 断言
type KnownField = 'start' | 'end'

/** 构造 el-form-item 形态的最小 DOM：根 + 原生 input（select 场景在用例内自行替换） */
function makeFieldDom(): HTMLElement {
  const root = document.createElement('div')
  root.className = 'el-form-item'
  root.appendChild(document.createElement('input'))
  return root
}

/** EP FormItem context 经 reactive 解包后，validateState / validateMessage 是 string 不是 ref */
function makeFormRef() {
  const fields: Record<string, FieldStub> = {}
  const fieldOf = (prop: string) => {
    if (!fields[prop]) {
      fields[prop] = {
        validateState: '',
        validateMessage: '',
        clearValidate: vi.fn(() => {
          fields[prop]!.validateState = ''
          fields[prop]!.validateMessage = ''
        }),
        $el: makeFieldDom(),
        labelId: `el-form-item-label-${Object.keys(fields).length + 1}`,
      }
    }
    return fields[prop]!
  }
  return {
    getField: vi.fn((prop: string) => fieldOf(prop)),
    clearValidate: vi.fn((props?: string[]) => {
      if (!props || props.length === 0) {
        for (const f of Object.values(fields)) f.clearValidate()
      } else {
        for (const p of props) fieldOf(p).clearValidate()
      }
    }),
    _fields: fields,
  } as unknown as {
    getField: (prop: string) => FieldStub | undefined
    clearValidate: (props?: string[] | string) => void
    _fields: Record<KnownField, FieldStub>
  }
}

/** 把字段 DOM 换成 el-select 形态：无原生 input，只有 .el-select__wrapper 包裹器 */
function asSelectField(field: FieldStub): HTMLElement {
  field.$el.innerHTML = ''
  const wrapper = document.createElement('div')
  wrapper.className = 'el-select__wrapper'
  field.$el.appendChild(wrapper)
  return wrapper
}

describe('formFieldError', () => {
  it('空 errors 不调用任何 API', () => {
    const form = makeFormRef()
    setFormFieldErrors(form as never, {})
    expect(form.getField).not.toHaveBeenCalled()
  })

  it('给每个字段写 error 态 + 消息', () => {
    const form = makeFormRef()
    setFormFieldErrors(form as never, { start: '必须 ≤ end', end: '必须 ≥ start' })
    // reactive 自动解包 ref，直接赋值 string
    expect(form._fields.start.validateState).toBe('error')
    expect(form._fields.start.validateMessage).toBe('必须 ≤ end')
    expect(form._fields.end.validateState).toBe('error')
    expect(form._fields.end.validateMessage).toBe('必须 ≥ start')
  })

  it('设置错误后控件获得 aria-invalid 与 aria-describedby 关联', async () => {
    const form = makeFormRef()
    form.getField('start')
    // 错误节点真实由 EP 在 validateMessage 写入后经 v-if 渲染，桩里提前放好模拟渲染完成
    const msg = document.createElement('div')
    msg.className = 'el-form-item__error'
    form._fields.start.$el.appendChild(msg)

    setFormFieldErrors(form as never, { start: '必须 ≤ end' })
    await nextTick()

    const input = form._fields.start.$el.querySelector('input')!
    expect(input.getAttribute('aria-invalid')).toBe('true')
    expect(msg.id).toBe(`${form._fields.start.labelId}-error`)
    expect(input.getAttribute('aria-describedby')).toBe(msg.id)
  })

  it('空字符串值清除该字段错误与 ARIA 关联', async () => {
    const form = makeFormRef()
    setFormFieldErrors(form as never, { start: '有错误' })
    await nextTick()
    const input = form._fields.start.$el.querySelector('input')!
    expect(form._fields.start.validateState).toBe('error')
    expect(input.getAttribute('aria-invalid')).toBe('true')

    // 再设空串清除
    setFormFieldErrors(form as never, { start: '' })
    expect(form._fields.start.clearValidate).toHaveBeenCalled()
    // 清除是同步的：属性立即摘掉
    expect(input.hasAttribute('aria-invalid')).toBe(false)
    expect(input.hasAttribute('aria-describedby')).toBe(false)
    // 前一次 set 挂起的 nextTick 回调不能把属性加回去（set→clear 连发的竞态）
    await nextTick()
    expect(input.hasAttribute('aria-invalid')).toBe(false)
  })

  it('el-select 等无原生 input 的字段，aria-invalid 落在包裹器上', async () => {
    const form = makeFormRef()
    form.getField('end')
    const wrapper = asSelectField(form._fields.end)
    const msg = document.createElement('div')
    msg.className = 'el-form-item__error'
    form._fields.end.$el.appendChild(msg)

    setFormFieldErrors(form as never, { end: '必填' })
    await nextTick()

    expect(wrapper.getAttribute('aria-invalid')).toBe('true')
    expect(wrapper.getAttribute('aria-describedby')).toBe(`${form._fields.end.labelId}-error`)
  })

  it('clearFormFieldErrors 带 props 时逐字段摘除 ARIA 关联', async () => {
    const form = makeFormRef()
    setFormFieldErrors(form as never, { start: 'err' })
    await nextTick()
    const input = form._fields.start.$el.querySelector('input')!
    expect(input.hasAttribute('aria-invalid')).toBe(true)

    clearFormFieldErrors(form as never, ['start'])
    expect(input.hasAttribute('aria-invalid')).toBe(false)
    // wire 的挂起回调也不得回填
    await nextTick()
    expect(input.hasAttribute('aria-invalid')).toBe(false)
  })

  it('$el 缺失时 ARIA 步骤静默跳过（防御）', () => {
    const form = makeFormRef()
    form.getField('start')
    ;(form._fields.start as unknown as { $el?: HTMLElement }).$el = undefined
    expect(() => setFormFieldErrors(form as never, { start: 'err' })).not.toThrow()
  })

  it('getField 不存在的 prop 静默跳过', () => {
    const form = makeFormRef()
    const spy = vi.spyOn(form, 'getField')
    setFormFieldErrors(form as never, { missing: 'err' })
    expect(spy).toHaveBeenCalledWith('missing')
  })

  it('clearFormFieldErrors 不传 props 时清除全部', () => {
    const form = makeFormRef()
    clearFormFieldErrors(form as never)
    expect(form.clearValidate).toHaveBeenCalledWith()
  })

  it('clearFormFieldErrors 传 props 数组时清除指定字段', () => {
    const form = makeFormRef()
    clearFormFieldErrors(form as never, ['start', 'end'])
    expect(form.clearValidate).toHaveBeenCalledWith(['start', 'end'])
  })

  it('FormInstance 没暴露 getField 时静默跳过（防御）', () => {
    const form = { clearValidate: vi.fn() } as unknown as Parameters<typeof setFormFieldErrors>[0]
    // 不抛错
    setFormFieldErrors(form, { start: 'err' })
  })
})
