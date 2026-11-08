// QueueConfigTab.vue 单元测试：队列行数上限 + 重启提示。
// 淘汰线在存储层初始化时固定，因此页面必须明示「需重启」，避免用户以为保存即生效。
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import QueueConfigTab from './QueueConfigTab.vue'

function mountTab(maxRows = 1_000_000) {
  const queue = { max_rows: maxRows }
  return { queue, wrapper: mount(QueueConfigTab, { props: { modelValue: queue } }) }
}

describe('QueueConfigTab.vue', () => {
  it('展示行数上限与重启提示', () => {
    const { wrapper } = mountTab()
    expect(wrapper.text()).toContain('队列行数上限')
    expect(wrapper.text()).toContain('修改后需重启网关')
    expect(wrapper.find('.el-input-number').exists()).toBe(true)
    wrapper.unmount()
  })

  it('修改上限后写回配置对象', async () => {
    const { queue, wrapper } = mountTab()
    const input = wrapper.find('input')
    await input.setValue('2000000')
    await input.trigger('change')
    expect(queue.max_rows).toBe(2_000_000)
    wrapper.unmount()
  })
})
