import { describe, it, expect } from 'vitest'
import { mount, config } from '@vue/test-utils'
import ConnectivityBanner from './ConnectivityBanner.vue'

// 组件内的 <router-link> 只在队列积压时真正渲染；无 router 的测试环境统一 stub，避免解析警告刷屏
config.global.stubs['router-link'] = { template: '<a><slot /></a>' }

/** W8：全局连通性横幅 —— 断连时明确告知「看到的是历史数据」 */
describe('ConnectivityBanner', () => {
  it('一切正常时不渲染任何内容', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { unreachable: false, degraded: false, staleSeconds: 3, failStreak: 0 },
    })
    expect(wrapper.find('.conn-banner').exists()).toBe(false)
  })

  it('单次失败不提示（避免交换机瞬断导致横幅闪烁）', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { unreachable: true, failStreak: 1, staleSeconds: 12 },
    })
    expect(wrapper.find('.conn-banner').exists()).toBe(false)
  })

  it('连续 2 次失败后提示失联，并说明数据停留在多久之前', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { unreachable: true, failStreak: 2, staleSeconds: 125 },
    })
    const banner = wrapper.find('.conn-banner')
    expect(banner.exists()).toBe(true)
    expect(banner.classes()).toContain('conn-danger')
    expect(wrapper.find('.conn-title').text()).toBe('无法连接网关')
    expect(wrapper.find('.conn-detail').text()).toBe('页面数据停留在 2 分钟前，不是实时值。')
  })

  it('从未成功连接过时，明确说明没有实时数据（而不是「0 秒前」）', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { unreachable: true, failStreak: 5, staleSeconds: null },
    })
    expect(wrapper.find('.conn-detail').text()).toBe('尚未成功连接过网关，当前页面没有实时数据。')
  })

  it('可达但组件降级时显示警告色与组件明细', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { degraded: true, components: ['mqtt: down', 'storage: degraded'] },
    })
    const banner = wrapper.find('.conn-banner')
    expect(banner.classes()).toContain('conn-warning')
    expect(wrapper.find('.conn-title').text()).toBe('网关部分组件异常')
    expect(wrapper.find('.conn-detail').text()).toBe('以下组件未就绪：mqtt: down、storage: degraded')
  })

  it('失联时提供「立即重试」，点击向外抛 retry 事件', async () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { unreachable: true, failStreak: 3, staleSeconds: 40 },
    })
    const btn = wrapper.find('.conn-retry')
    expect(btn.exists()).toBe(true)
    await btn.trigger('click')
    expect(wrapper.emitted('retry')).toHaveLength(1)
  })

  it('队列积压时附具体条数，并提供「查看断连续传」链接', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { degraded: true, components: ['采集队列：队列积压'], queueLen: 126484 },
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    expect(wrapper.find('.conn-detail').text()).toContain('采集队列：队列积压 126,484 条')
    expect(wrapper.find('.conn-link').exists()).toBe(true)
  })

  it('队列积压但快照无 queue_len（老后端）时降级为无数值文案', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { degraded: true, components: ['采集队列：队列积压'] },
      global: { stubs: { RouterLink: { template: '<a><slot /></a>' } } },
    })
    expect(wrapper.find('.conn-detail').text()).toContain('采集队列：队列积压')
    expect(wrapper.find('.conn-detail').text()).not.toContain('条')
    // 链接仍在，引导用户去断连续传页看实时数值
    expect(wrapper.find('.conn-link').exists()).toBe(true)
  })

  it('非队列积压组件不显示断连续传链接', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { degraded: true, components: ['mqtt: down'] },
    })
    expect(wrapper.find('.conn-link').exists()).toBe(false)
  })

  it('降级时不显示重试按钮（重试必然仍是降级）', () => {
    const wrapper = mount(ConnectivityBanner, {
      props: { degraded: true, components: ['mqtt: down'] },
    })
    expect(wrapper.find('.conn-retry').exists()).toBe(false)
  })
})
