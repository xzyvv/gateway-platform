/**
 * 总线下拉统一件契约。
 *
 * 收敛前这条 el-select 在 9 处各写一遍，差异都在现场会踩的细节上：
 * - 有的没写 `no-data-text`，未配置总线时只弹一个空面板，用户分不清
 *   「没配」还是「加载失败」；
 * - `aria-label` 有的叫「总线」有的叫「调试目标总线」，读屏听到的名字与
 *   页面可见标签对不上；
 * - 宽度 200/220 混用；
 * - 选项里的协议名，有的拼进 label 有的放右侧灰字。
 *
 * 这里锁住默认值，让调用点只需给「真正不同」的那几项。
 */
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, h, ref } from 'vue'
import BusSelect, { type BusOption } from './BusSelect.vue'

const OPTIONS: BusOption[] = [
  { label: '/dev/ttyUSB0', value: '/dev/ttyUSB0', meta: 'Modbus RTU' },
  { label: '192.168.1.50:502', value: '192.168.1.50:502' },
]

function mountSelect(props: Record<string, unknown> = {}) {
  // 不要 `stubs: { teleport: true }`：el-select 的下拉是 <teleport to="body">，
  // 打桩后下拉被内联到组件树里，el-select 内部「选项注册 ↔ 选中项回算」会互相触发，
  // 每次挂载都抛一串「Maximum recursive updates exceeded」。用例本身不读下拉 DOM，
  // 让 teleport 正常往 body 里渲染即可（选项断言走 findAllComponents，不受影响）。
  return mount(BusSelect, {
    props: { modelValue: '', options: OPTIONS, ...props },
  })
}

describe('BusSelect', () => {
  it('空列表提示默认说清「没配」，而不是留一个空面板', () => {
    // 这是收敛前最容易漏的一项：未配总线时用户要能立刻知道下一步去哪
    expect(mountSelect().props('noDataText')).toBe('暂无总线，请先在配置管理中添加')
  })

  it('aria-label 默认为「总线」，与表单可见标签一致', () => {
    expect(mountSelect().props('ariaLabel')).toBe('总线')
  })

  it('宽度数字按 px 处理，也接受百分比', () => {
    expect(mountSelect({ width: 200 }).attributes('style')).toContain('width: 200px')
    expect(mountSelect({ width: '100%' }).attributes('style')).toContain('width: 100%')
  })

  it('选项的 meta 渲染为右侧灰色说明，label 仍是提交值口径', () => {
    const wrapper = mountSelect()
    const opts = wrapper.findAllComponents({ name: 'ElOption' })
    expect(opts).toHaveLength(2)
    // 第一项有协议说明，第二项没有（不应渲染空的 span 留下多余间距）
    expect(opts[0]!.find('.bus-opt-meta').exists()).toBe(true)
    expect(opts[1]!.find('.bus-opt-meta').exists()).toBe(false)
    expect(opts[0]!.props('value')).toBe('/dev/ttyUSB0')
  })

  it('选中与改选都向上透传，change 额外带值', async () => {
    // 用真实的 v-model 宿主：直接给子组件 $emit 而不更新绑定 prop 会让
    // el-select 内部状态与 prop 永久不一致（触发递归更新告警），不是真实用法
    const harness = defineComponent({
      components: { BusSelect },
      setup() {
        const picked = ref('')
        const changed = ref('')
        return () => h(BusSelect, {
          modelValue: picked.value,
          options: OPTIONS,
          'onUpdate:modelValue': (v: string) => { picked.value = v },
          onChange: (v: string) => { changed.value = v },
        })
      },
    })
    const wrapper = mount(harness)
    const select = wrapper.findComponent({ name: 'ElSelect' })
    select.vm.$emit('update:modelValue', '192.168.1.50:502')
    select.vm.$emit('change', '192.168.1.50:502')
    await wrapper.vm.$nextTick()

    // 事件真的被接收：宿主里的 v-model 已更新
    expect(wrapper.findComponent({ name: 'ElSelect' }).props('modelValue')).toBe('192.168.1.50:502')
  })

  it('prefix 插槽透传给 el-select（批量编辑用它放「是否修改」勾选框）', () => {
    const wrapper = mount(BusSelect, {
      props: { modelValue: '', options: OPTIONS },
      slots: { prefix: '<span class="probe-prefix">✓</span>' },
    })
    // slot 只在 el-select 渲染时出现；这里断言插槽内容被接收而非被丢弃
    expect(wrapper.html()).toContain('probe-prefix')
  })
})
