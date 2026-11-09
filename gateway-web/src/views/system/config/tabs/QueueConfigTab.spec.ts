// QueueConfigTab.vue 单元测试：队列行数上限 + 重启提示。
// 淘汰线在存储层初始化时固定，因此页面必须明示「需重启」，避免用户以为保存即生效。
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElSelect } from 'element-plus'
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
    // 档位下拉（而非裸数字框）：首屏直接给出常用档位
    expect(wrapper.findComponent(ElSelect).exists()).toBe(true)
    wrapper.unmount()
  })

  it('修改上限后写回配置对象', async () => {
    const { queue, wrapper } = mountTab()
    // allow-create 输出的是字符串，组件需转成数字写回
    wrapper.findComponent(ElSelect).vm.$emit('update:modelValue', '2000000')
    await nextTick()
    expect(queue.max_rows).toBe(2_000_000)
    wrapper.unmount()
  })

  it('输入的越界值收敛到后端校验区间', async () => {
    const { queue, wrapper } = mountTab()
    const select = wrapper.findComponent(ElSelect)
    select.vm.$emit('update:modelValue', '1')
    await nextTick()
    expect(queue.max_rows).toBe(200_000)

    select.vm.$emit('update:modelValue', '999999999')
    await nextTick()
    expect(queue.max_rows).toBe(50_000_000)
    wrapper.unmount()
  })
})
