/**
 * 日志面板公共件契约。
 *
 * 这个组件存在的理由不是「少写几行模板」，而是三份面板的三件套此前各缺一块：
 * 执行日志不能导出、操作记录不能复制全部、测试日志只有清空。现场的直接后果是
 * 「跑完一轮只能截图」。所以这里锁两件事：
 *   1. `copyable / exportable / clearable` 打开哪个就渲染哪个，事件真的抛出去；
 *   2. 空内容时复制/导出必须禁用——导出一个只有表头的 CSV 会让现场以为
 *      「日志丢了」，比按钮灰着更糟。
 */
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import LogPanel from './LogPanel.vue'

function mountPanel(props: Record<string, unknown> = {}) {
  return mount(LogPanel, {
    props: {
      title: '执行日志',
      empty: false,
      emptyText: '点击「单步执行」开始调试',
      ...props,
    },
    slots: { default: '<div class="probe-line">第 1 步 成功</div>' },
  })
}

/** 按文字找按钮（按钮内含图标节点，textContent 前后可能有空白） */
function button(wrapper: ReturnType<typeof mountPanel>, text: string) {
  return wrapper.findAll('button').find((b) => b.text().includes(text))
}

describe('LogPanel', () => {
  it('默认不渲染任何操作按钮：由使用方按需打开', () => {
    const wrapper = mountPanel()
    expect(wrapper.findAll('button')).toHaveLength(0)
  })

  it('三件套按开关渲染，且点击后各自抛出事件', async () => {
    const wrapper = mountPanel({ copyable: true, exportable: true, clearable: true })

    await button(wrapper, '复制全部')!.trigger('click')
    await button(wrapper, '导出 CSV')!.trigger('click')
    await button(wrapper, '清空')!.trigger('click')

    expect(wrapper.emitted('copy-all')).toHaveLength(1)
    expect(wrapper.emitted('export')).toHaveLength(1)
    expect(wrapper.emitted('clear')).toHaveLength(1)
  })

  it('只打开一个开关时只出现一个按钮', () => {
    const wrapper = mountPanel({ exportable: true })
    expect(wrapper.findAll('button')).toHaveLength(1)
    expect(button(wrapper, '导出 CSV')).toBeTruthy()
  })

  it('空内容时渲染空态文案，并禁用复制/导出（导出一个只有表头的 CSV 更像故障）', () => {
    const wrapper = mountPanel({ empty: true, copyable: true, exportable: true, clearable: true })

    expect(wrapper.text()).toContain('点击「单步执行」开始调试')
    expect(wrapper.find('.probe-line').exists()).toBe(false)
    expect(button(wrapper, '复制全部')!.attributes('disabled')).toBeDefined()
    expect(button(wrapper, '导出 CSV')!.attributes('disabled')).toBeDefined()
    // 清空本身也是空操作，同样禁用，避免按钮看起来可点却没反应
    expect(button(wrapper, '清空')!.attributes('disabled')).toBeDefined()
  })

  it('有内容时渲染默认插槽，不再显示空态', () => {
    const wrapper = mountPanel()
    expect(wrapper.find('.probe-line').text()).toBe('第 1 步 成功')
    expect(wrapper.text()).not.toContain('点击「单步执行」开始调试')
  })

  it('滚动区按 maxHeight 限高，页面不会被长日志撑长', () => {
    const wrapper = mountPanel({ maxHeight: 560 })
    expect(wrapper.find('.log-panel-scroll').attributes('style')).toContain('max-height: 560px')
  })

  it('空态保留最小高度，卡片不会塌成一条', () => {
    const wrapper = mountPanel({ empty: true, emptyMinHeight: 160 })
    expect(wrapper.find('.log-panel-scroll').attributes('style')).toContain('min-height: 160px')
  })
})
