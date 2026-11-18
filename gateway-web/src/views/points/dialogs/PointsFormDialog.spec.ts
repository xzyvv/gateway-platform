import { describe, it, expect, beforeEach, vi, type Mock } from 'vitest'
import { mount, flushPromises, type VueWrapper } from '@vue/test-utils'
import { defineComponent, h, provide, reactive, ref, nextTick, type Ref } from 'vue'
import type { FormInstance, FormRules } from 'element-plus'
import PointsFormDialog from './PointsFormDialog.vue'
import ModbusAddressInput from '@/components/industrial/ModbusAddressInput.vue'
import { pointFormContextKey } from '../pointFormContext'
import type { PointForm } from '@/composables/devices/usePoints'
import type { DialogDirtyGuard } from '@/composables/core/useDialogDirtyGuard'

/**
 * 单点表单弹窗（PointsFormDialog.vue）的回归护栏。
 *
 * 这套用例守的是 PointList.vue 拆分时最容易悄悄断掉的三条链路：
 * 1. 表单字段的 v-model 写回**注入进来的同一个 reactive 对象**
 *    （表单模型若改成 props 下发，`vue/no-mutating-props` 会直接拦下改字段的写法）；
 * 2. `<el-form ref="pointFormRef">` 的实例能回写进注入的 Ref——
 *    composable 的 submitPoint() 依赖 `pointFormRef.value?.validate()`，断了就提交不了；
 * 3. 提交 / 取消 / 脏守卫仍接到页面提供的那套回调上。
 */
function createPointForm(): PointForm {
  return {
    id: '', name: '', bus: 'bus1', protocol: 'modbus', address: '', slave_id: 1, address_num: 0,
    interval: 1, writable: false, unit: '', scale: 1, offset: 0,
    ct_ratio: null, pt_ratio: null, function_code: 3, data_type: '', byte_order: '',
    register_count: null, formula: '', deadband: 0, on_change: false,
    alarm_high: undefined, alarm_low: undefined, aggregation_fn: '', aggregation_window: 60,
  }
}

let pointForm: PointForm
let pointFormRef: Ref<FormInstance | undefined>
let showForm: Ref<boolean>
let formMode: Ref<'create' | 'edit'>
let submitting: Ref<boolean>
let currentDeviceName: Ref<string>
let northInterval: Ref<number>
let pointRules: FormRules
let formGuard: DialogDirtyGuard
let guardClose: Mock<() => Promise<void>>
let submitPoint: Mock<() => Promise<void>>
let resetForm: Mock<() => void>

beforeEach(() => {
  vi.clearAllMocks()
  pointForm = reactive<PointForm>(createPointForm())
  pointFormRef = ref<FormInstance>()
  showForm = ref(true)
  formMode = ref<'create' | 'edit'>('create')
  submitting = ref(false)
  currentDeviceName = ref('dianbiao1')
  northInterval = ref(60)
  pointRules = {}
  guardClose = vi.fn<() => Promise<void>>(async () => {})
  formGuard = {
    beforeClose: vi.fn(),
    close: guardClose,
    confirmClose: vi.fn(async () => true),
    isDirty: () => false,
    markClean: vi.fn(),
  }
  submitPoint = vi.fn<() => Promise<void>>(async () => {})
  resetForm = vi.fn<() => void>(() => {})
})

/** 用一个宿主组件提供上下文：与真实页面（PointList.vue）走同一条 provide/inject 链路 */
function mountDialog(): VueWrapper {
  const Host = defineComponent({
    setup() {
      provide(pointFormContextKey, {
        pointForm, pointRules, pointFormRef, formMode, submitting, showForm,
        currentDeviceName, northInterval,
        protocolLabel: { modbus: 'Modbus', dlt645: 'DLT645' },
        formGuard, submitPoint, resetForm,
      })
      return () => h(PointsFormDialog)
    },
  })
  return mount(Host)
}

/** 按 el-form-item 的 label 找表单项（Element Plus 会生成 label + 控件的关联） */
function itemByLabel(wrapper: VueWrapper, label: string) {
  return wrapper.findAll('.el-form-item').find((item) => {
    const l = item.find('.el-form-item__label')
    return l.exists() && l.text().includes(label)
  })
}

describe('PointsFormDialog.vue', () => {
  it('弹窗打开后表单渲染，字段回写到注入的同一个表单对象', async () => {
    const wrapper = mountDialog()
    await flushPromises()

    expect(wrapper.text()).toContain('新增点位')
    const idInput = itemByLabel(wrapper, '点位 ID')!.find('input')
    await idInput.setValue('temp_01')

    // 改的是注入进来的同一个 reactive 对象（不是副本）
    expect(pointForm.id).toBe('temp_01')
  })

  it('el-form 实例回写进注入的 pointFormRef，submitPoint 的校验链路不断', async () => {
    mountDialog()
    await flushPromises()

    // 断了这一条，composable 的 submitPoint() 拿不到表单实例，提交会静默失败
    expect(pointFormRef.value).toBeTruthy()
    expect(typeof pointFormRef.value!.validate).toBe('function')
    expect(typeof pointFormRef.value!.clearValidate).toBe('function')
  })

  it('提交与取消分别接到 submitPoint 与未保存守卫', async () => {
    const wrapper = mountDialog()
    await flushPromises()

    const buttons = wrapper.findAll('button')
    const cancel = buttons.find((b) => b.text().trim() === '取消')
    const submit = buttons.find((b) => b.text().trim() === '创建')
    expect(cancel, '应能找到取消按钮').toBeTruthy()
    expect(submit, '应能找到提交按钮').toBeTruthy()

    await cancel!.trigger('click')
    expect(guardClose).toHaveBeenCalled()

    await submit!.trigger('click')
    expect(submitPoint).toHaveBeenCalled()
  })

  it('总线与协议标注继承自设备，协议名取注入的标签表', async () => {
    pointForm.protocol = 'modbus'
    const wrapper = mountDialog()
    await flushPromises()

    expect(wrapper.text()).toContain('继承自设备「dianbiao1」')
    expect(wrapper.text()).toContain('协议继承自设备，不可单独修改')
    // 协议标签表来自上下文（注入的是 { modbus: 'Modbus' }）
    const protocolInput = itemByLabel(wrapper, '协议')!.find('input')
    expect((protocolInput.element as HTMLInputElement).value).toBe('Modbus')
  })

  it('Modbus 走区段寻址输入，其它协议走普通地址输入', async () => {
    const wrapper = mountDialog()
    await flushPromises()
    expect(wrapper.findComponent(ModbusAddressInput).exists(), 'Modbus 应走区段输入').toBe(true)

    pointForm.protocol = 's7comm'
    await nextTick()
    expect(wrapper.findComponent(ModbusAddressInput).exists(), '非 Modbus 不应走区段输入').toBe(false)
  })

  it('变比按单位决定 CT/PT 是否可填', async () => {
    const wrapper = mountDialog()
    await flushPromises()

    // 电压单位：只有 PT 生效，CT 置灰
    pointForm.unit = 'V'
    await nextTick()
    expect(itemByLabel(wrapper, 'CT 变比')!.find('input').attributes('disabled')).toBeDefined()
    expect(itemByLabel(wrapper, 'PT 变比')!.find('input').attributes('disabled')).toBeUndefined()

    // 功率因数：两者都不生效
    pointForm.unit = 'PF'
    await nextTick()
    expect(itemByLabel(wrapper, 'CT 变比')!.find('input').attributes('disabled')).toBeDefined()
    expect(itemByLabel(wrapper, 'PT 变比')!.find('input').attributes('disabled')).toBeDefined()
  })

  it('采集周期上界跟随北向上报周期', async () => {
    northInterval.value = 30
    const wrapper = mountDialog()
    await flushPromises()

    // 上界 = northInterval - 1；提示语里回显当前上报周期
    expect(wrapper.text()).toContain('当前 30 秒')
    expect(wrapper.text()).toContain('必须小于北向上报周期')
  })
})
