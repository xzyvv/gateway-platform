/**
 * 批量操作条的契约。
 *
 * 它承担两个容易做错的点：
 *   1. 0 选中时必须完全不存在（不是隐藏的空壳），避免工具栏被常驻占位
 *   2. 跨页勾选时要显式提示作用范围，否则用户会以为只作用于当前页
 */
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { h } from 'vue'
import SelectionBar from './SelectionBar.vue'

function mountBar(props: { count: number; hint?: string }) {
  return mount(SelectionBar, {
    props,
    slots: { default: () => h('button', { class: 'slot-action' }, '批量删除') },
  })
}

describe('SelectionBar', () => {
  it('未选中时不渲染任何内容', () => {
    const wrapper = mountBar({ count: 0 })
    expect(wrapper.find('.selection-bar').exists()).toBe(false)
    // slot 内容也不应渲染（否则按钮仍占位）
    expect(wrapper.find('.slot-action').exists()).toBe(false)
  })

  it('选中后显示数量、动作插槽与取消选择', () => {
    const wrapper = mountBar({ count: 3 })
    const bar = wrapper.get('.selection-bar')
    expect(bar.text()).toContain('已选')
    expect(bar.text()).toContain('3')
    expect(bar.text()).toContain('项')
    expect(wrapper.find('.slot-action').exists()).toBe(true)
    expect(bar.text()).toContain('取消选择')
  })

  it('提供 hint 时展示选择范围提示', () => {
    const wrapper = mountBar({ count: 5, hint: '可跨页勾选，操作作用于全部已选' })
    expect(wrapper.get('.sb-hint').text()).toContain('可跨页勾选')
  })

  it('未提供 hint 时不渲染提示元素', () => {
    expect(mountBar({ count: 1 }).find('.sb-hint').exists()).toBe(false)
  })

  it('点击取消选择 emit clear', async () => {
    const wrapper = mountBar({ count: 2 })
    const clear = wrapper.findAll('button').find((b) => b.text().includes('取消选择'))
    expect(clear).toBeTruthy()
    await clear!.trigger('click')
    expect(wrapper.emitted('clear')).toHaveLength(1)
  })
})