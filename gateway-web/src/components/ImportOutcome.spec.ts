/**
 * 导入结果面板契约（R1）。
 *
 * 三条导入路径共用：失败原因必须是后端原文，且「未发生导入」时不占位。
 */
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import ImportOutcome from './ImportOutcome.vue'

describe('ImportOutcome', () => {
  it('visible=false 时不渲染任何内容（导入尚未发生）', () => {
    const wrapper = mount(ImportOutcome, {
      props: { visible: false, success: true, successTitle: '导入成功' },
    })
    expect(wrapper.find('.import-outcome').exists()).toBe(false)
  })

  it('成功时渲染成功标题与补充说明', () => {
    const wrapper = mount(ImportOutcome, {
      props: { visible: true, success: true, successTitle: '配置已热重载', subtitle: '新增 2 条、覆盖 1 条' },
    })
    expect(wrapper.text()).toContain('配置已热重载')
    expect(wrapper.text()).toContain('新增 2 条、覆盖 1 条')
  })

  it('失败时标题直接透传后端中文原因', () => {
    const wrapper = mount(ImportOutcome, {
      props: { visible: true, success: false, error: '第 3 行 JSON 语法错误' },
    })
    expect(wrapper.text()).toContain('导入失败：第 3 行 JSON 语法错误')
  })

  it('失败但无原因时给出兜底文案，不显示空白标题', () => {
    const wrapper = mount(ImportOutcome, { props: { visible: true, success: false } })
    expect(wrapper.text()).toContain('导入失败：未知错误')
  })

  it('成功时忽略 error，不把上一次的失败原因带出来', () => {
    const wrapper = mount(ImportOutcome, {
      props: { visible: true, success: true, successTitle: '片段已合并导入', error: '旧错误' },
    })
    expect(wrapper.text()).toContain('片段已合并导入')
    expect(wrapper.text()).not.toContain('旧错误')
  })

  it('extra 槽渲染附加动作（如「查看配置版本」）', () => {
    const wrapper = mount(ImportOutcome, {
      props: { visible: true, success: true, successTitle: '已导入' },
      slots: { extra: '<button class="extra-btn">查看配置版本</button>' },
    })
    expect(wrapper.find('.import-outcome-actions .extra-btn').exists()).toBe(true)
  })
})
