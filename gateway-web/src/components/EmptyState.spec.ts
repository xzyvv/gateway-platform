/**
 * 空态的契约：不只是"没有数据"的说明，还要给出下一步动作。
 *
 * 这里同时锁住 imageSize 的归一化：历史上调用点写的是字符串 "80"，
 * element-plus 会对每个实例打印 "Invalid prop: type check failed"，
 * 30+ 处调用点叠加后在控制台刷屏。组件内统一转成 number。
 */
import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import EmptyState from './EmptyState.vue'

describe('EmptyState', () => {
  it('仅给描述时不渲染按钮', () => {
    const wrapper = mount(EmptyState, { props: { description: '暂无数据' } })
    expect(wrapper.text()).toContain('暂无数据')
    expect(wrapper.find('button').exists()).toBe(false)
  })

  it('actionText 渲染主按钮并 emit action', async () => {
    const wrapper = mount(EmptyState, {
      props: { description: '暂无设备', actionText: '立即新增' },
    })
    const btn = wrapper.get('button')
    expect(btn.text()).toContain('立即新增')
    await btn.trigger('click')
    expect(wrapper.emitted('action')).toHaveLength(1)
  })

  it('actionType 默认为 primary，可降级为 default', () => {
    const primary = mount(EmptyState, { props: { actionText: '新增' } })
    expect(primary.get('button').classes()).toContain('el-button--primary')
    const plain = mount(EmptyState, { props: { actionText: '新增', actionType: 'default' } })
    expect(plain.get('button').classes()).not.toContain('el-button--primary')
  })

  it('imageSize 传字符串数字时归一化为 number，不触发 prop 类型告警', () => {
    const spy = vi.spyOn(console, 'warn').mockImplementation(() => {})
    mount(EmptyState, { props: { description: '空', imageSize: '80' } })
    const propWarnings = spy.mock.calls.filter((c) => String(c[0]).includes('Invalid prop'))
    expect(propWarnings).toHaveLength(0)
    spy.mockRestore()
  })
})