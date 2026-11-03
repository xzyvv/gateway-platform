/**
 * 组件健康表的契约。
 *
 * 这张表现在同时挂在运行总览页与「系统 → 网关信息」页上，
 * 所以它必须自己保证两件事：
 *   1. 组件名 / 状态名走 utils/healthStatus 的中文词表（英文原词不许漏到界面上）；
 *   2. 样式自带（表格单元格是 render 函数 h() 出来的元素，拿不到页面的 scope id，
 *      样式写在页面 scoped css 里等于不生效）。
 */
import { describe, it, expect } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import HealthStatus from './HealthStatus.vue'

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('HealthStatus.vue', () => {
  it('渲染组件中文名与中文状态徽章', async () => {
    const wrapper = mount(HealthStatus, {
      props: {
        healthRows: [
          { name: 'mqtt', status: 'down' },
          { name: 'process', status: 'ok' },
        ],
      },
    })
    await settle()
    const text = wrapper.text()
    expect(text).toContain('组件健康状态')
    expect(text).toContain('MQTT 客户端')
    expect(text).toContain('不可用')
    expect(text).toContain('网关主进程')
    expect(text).toContain('正常')
    wrapper.unmount()
  })

  it('没有组件数据时显示空态而不是空白表格', async () => {
    const wrapper = mount(HealthStatus, { props: { healthRows: [] } })
    await settle()
    expect(wrapper.text()).toContain('暂无组件状态数据')
    wrapper.unmount()
  })

  it('组件自带 scoped 标记（样式跟着组件走，不依赖使用方页面的 css）', async () => {
    const wrapper = mount(HealthStatus, {
      props: { healthRows: [{ name: 'mqtt', status: 'ok' }] },
    })
    await settle()
    const name = wrapper.find('.health-name')
    expect(name.exists()).toBe(true)
    expect(Object.keys(name.attributes()).some((k) => k.startsWith('data-v-'))).toBe(true)
    wrapper.unmount()
  })
})
