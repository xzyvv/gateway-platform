import { describe, it, expect } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import ChannelTestForm from './ChannelTestForm.vue'
import type { ChannelRow } from './types'

/** 构造一个最小合法的通道行（满足 NorthChannel 必填字段），可覆盖测试状态字段 */
function makeChannel(over: Partial<ChannelRow> = {}): ChannelRow {
  return {
    name: 'mqtt',
    type: 'mqtt',
    enabled: true,
    connected: true,
    queue_length: 0,
    extra: { broker: 'tcp://b:1883' },
    ...over,
  }
}

/** 等待 el-table 在挂载后完成 body 行渲染（jsdom 下需释放微任务 + 调度） */
async function settle() {
  await flushPromises()
  await nextTick()
}

describe('ChannelTestForm.vue — 上行通道测试表单', () => {
  it('渲染表头计数与通道名称行', async () => {
    const channels = [makeChannel({ name: 'mqtt' }), makeChannel({ name: 'webhook', type: 'http' })]
    const wrapper = mount(ChannelTestForm, { props: { channels, testing: false } })
    await settle()
    const text = wrapper.text()
    expect(text).toContain('上行通道（2）')
    expect(text).toContain('mqtt')
    expect(text).toContain('webhook')
  })

  it('通道列表为空时"测试全部通道"按钮禁用', () => {
    const wrapper = mount(ChannelTestForm, { props: { channels: [], testing: false } })
    const btn = wrapper.find('.ct-section-header button')
    expect(btn.attributes('disabled')).toBeDefined()
  })

  it('点击"测试全部通道"触发 test-all 事件', async () => {
    const wrapper = mount(ChannelTestForm, {
      props: { channels: [makeChannel()], testing: false },
    })
    const btn = wrapper.find('.ct-section-header button')
    await btn.trigger('click')
    const events = wrapper.emitted('test-all')
    expect(events).toBeTruthy()
    expect(events!.length).toBe(1)
  })

  it('点击单行"测试"按钮触发 test-one 事件并携带该通道行', async () => {
    const ch = makeChannel({ name: 'mqtt' })
    const wrapper = mount(ChannelTestForm, {
      props: { channels: [ch], testing: false },
    })
    await settle()
    const rowBtn = wrapper.findAll('button').find((b) => b.text().trim() === '测试')
    expect(rowBtn).toBeTruthy()
    await rowBtn!.trigger('click')
    const events = wrapper.emitted('test-one')!
    expect(events.length).toBe(1)
    // 子组件 emit 的 row 是 props 的响应式代理，与原始对象引用不同，故用结构相等
    expect(events[0]![0]).toStrictEqual(ch)
  })

  it('connected 状态渲染为 已连接 / 未连接 / 未知', async () => {
    const channels = [
      makeChannel({ name: 'c1', connected: true }),
      makeChannel({ name: 'c2', connected: false }),
      makeChannel({ name: 'c3', connected: null }),
    ]
    const wrapper = mount(ChannelTestForm, { props: { channels, testing: false } })
    await settle()
    const text = wrapper.text()
    expect(text).toContain('已连接')
    expect(text).toContain('未连接')
    expect(text).toContain('未知')
  })

  it('根据 _status 渲染 通过 / 失败 / 测试中 / 未测试，并展示 _detail 与 _error', async () => {
    const channels = [
      makeChannel({ name: 'c1', _status: 'ok', _detail: 'topic= factory/data' }),
      makeChannel({ name: 'c2', _status: 'fail', _error: 'broker 不可达' }),
      makeChannel({ name: 'c3', _status: 'testing' }),
      makeChannel({ name: 'c4', _status: 'idle' }),
    ]
    const wrapper = mount(ChannelTestForm, { props: { channels, testing: false } })
    await settle()
    const text = wrapper.text()
    expect(text).toContain('通过')
    expect(text).toContain('失败')
    expect(text).toContain('测试中')
    expect(text).toContain('未测试')
    expect(text).toContain('factory/data')
    expect(text).toContain('broker 不可达')
  })

  it('ok 且无 _detail 时回退展示"发送成功"', async () => {
    const channels = [makeChannel({ name: 'c1', _status: 'ok' })]
    const wrapper = mount(ChannelTestForm, { props: { channels, testing: false } })
    await settle()
    expect(wrapper.text()).toContain('发送成功')
  })

  it('testing=true 时"测试全部通道"按钮处于 loading 态', () => {
    const wrapper = mount(ChannelTestForm, {
      props: { channels: [makeChannel()], testing: true },
    })
    const btn = wrapper.find('.ct-section-header button')
    expect(btn.classes().some((c) => c.includes('loading'))).toBe(true)
  })
})