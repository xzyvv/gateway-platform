// MqttConfigTab.vue 单元测试：基础参数平铺 + 高级参数分组默认折叠。
// 折叠的意义：心跳/ACK/压缩/限速/Payload 模板属于调优项，多数部署用默认值即可，
// 折叠后首屏只留必填项，同时标题上给出当前值摘要，不必展开也能确认配置。
import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import MqttConfigTab from './MqttConfigTab.vue'
import { ElSelect } from 'element-plus'
import type { MqttConfig } from '@/api/types'

// 上报策略的引导链接用 useRouter().push 跳页；组件测试不挂真路由，这里替换为可断言的 spy。
const { pushMock } = vi.hoisted(() => ({ pushMock: vi.fn() }))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: pushMock }) }))

function makeMqtt(over: Partial<MqttConfig> = {}): MqttConfig {
  return {
    enabled: true,
    broker: 'mqtt://localhost:1883',
    topic: 'gateway/data',
    publish_interval: 5,
    qos: 1,
    keep_alive: 60,
    ack_timeout_secs: 10,
    username: null,
    password: null,
    compression: null,
    compression_threshold: null,
    resume_rate_limit: null,
    payload_template: null,
    tls: null,
    reconnect_backoff: { seconds: [2, 4, 8, 16, 32, 60], jitter_ratio: 0.4 },
    report_mode: 'stream',
    ...over,
  }
}

function mountTab(mqtt: MqttConfig = makeMqtt()) {
  return mount(MqttConfigTab, { props: { modelValue: mqtt } })
}

describe('MqttConfigTab.vue 高级参数分组', () => {
  it('基础连接参数平铺展示', () => {
    const wrapper = mountTab()
    const text = wrapper.text()
    for (const label of ['启用 MQTT', 'Broker 地址', 'Topic', 'QoS 等级', '上报周期', '上报策略', '用户名', '密码']) {
      expect(text, `基础字段缺失：${label}`).toContain(label)
    }
    wrapper.unmount()
  })

  it('高级参数默认折叠，标题上给出当前值摘要', () => {
    const wrapper = mountTab()
    expect(wrapper.text()).toContain('高级参数')
    // 折叠状态下摘要可见：心跳/ACK/压缩取值不必展开就能看到
    expect(wrapper.text()).toContain('心跳 60s')
    expect(wrapper.text()).toContain('ACK 10s')
    expect(wrapper.text()).toContain('压缩 不压缩')
    expect(wrapper.text()).toContain('退避 标准')

    const wrap = wrapper.find('.el-collapse-item__wrap')
    expect(wrap.exists()).toBe(true)
    expect(wrap.isVisible(), '高级参数默认应收起').toBe(false)
    wrapper.unmount()
  })

  it('展开高级分组后可见调优字段', async () => {
    const wrapper = mountTab()
    await wrapper.find('.el-collapse-item__header').trigger('click')
    await nextTick()

    const wrap = wrapper.find('.el-collapse-item__wrap')
    expect(wrap.isVisible(), '点击标题后高级参数应展开').toBe(true)
    const text = wrapper.text()
    for (const label of ['心跳间隔（秒）', 'ACK 超时（秒）', '压缩方式', '压缩阈值（字节）', '续传限速（条/秒）', 'CA 证书路径', '重连退避', 'Payload 模板']) {
      expect(text, `高级字段缺失：${label}`).toContain(label)
    }
    wrapper.unmount()
  })

  it('告警 Topic 由数据 Topic 派生，只读展示且不可编辑', () => {
    const wrapper = mountTab()
    const inputs = wrapper.findAll('.derived-topics input')
    expect(inputs.map((i) => (i.element as HTMLInputElement).value))
      .toEqual(['gateway/data/alarm', 'gateway/data/system/alarm'])
    for (const i of inputs) expect(i.attributes('readonly')).toBeDefined()
    wrapper.unmount()
  })

  it('展示值与后端派生规则逐字一致（不做斜杠归一化）', () => {
    const wrapper = mountTab(makeMqtt({ topic: 'gw/data/' }))
    const inputs = wrapper.findAll('.derived-topics input')
    // 后端是 format!("{}/alarm", topic) 直接拼接，展示必须同源，否则用户订阅不到
    expect(inputs.map((i) => (i.element as HTMLInputElement).value))
      .toEqual(['gw/data//alarm', 'gw/data//system/alarm'])
    wrapper.unmount()
  })

  it('重连退避提供三个预设，选择后写回对应档位', async () => {
    const mqtt = makeMqtt()
    const wrapper = mountTab(mqtt)
    const select = wrapper.find('.backoff-field').findComponent(ElSelect)
    // 默认配置（后端 default_reconnect_backoff）识别为「标准」
    expect(select.props('modelValue')).toBe('standard')

    select.vm.$emit('update:modelValue', 'fast')
    await nextTick()
    expect(mqtt.reconnect_backoff).toEqual({ seconds: [1, 1, 2, 3, 5, 10], jitter_ratio: 0.2 })
    expect(wrapper.find('.backoff-field .field-hint').text()).toContain('1 / 1 / 2 / 3 / 5 / 10')

    select.vm.$emit('update:modelValue', 'conservative')
    await nextTick()
    expect(mqtt.reconnect_backoff).toEqual({ seconds: [5, 15, 30, 60, 120, 300], jitter_ratio: 0.5 })
    wrapper.unmount()
  })

  it('非预设档位显示为「自定义」，选中它不改写现有配置', async () => {
    const mqtt = makeMqtt({ reconnect_backoff: { seconds: [3, 7, 20], jitter_ratio: 0 } })
    const wrapper = mountTab(mqtt)
    const select = wrapper.find('.backoff-field').findComponent(ElSelect)
    expect(select.props('modelValue')).toBe('custom')

    select.vm.$emit('update:modelValue', 'custom')
    await nextTick()
    expect(mqtt.reconnect_backoff).toEqual({ seconds: [3, 7, 20], jitter_ratio: 0 })
    wrapper.unmount()
  })

  it('上报策略卡片：选「变化上报」写回 stream 并展开配置引导', async () => {
    const mqtt = makeMqtt()
    const wrapper = mountTab(mqtt)
    // 默认 stream：保留两种模式的差异说明，选卡片之前也能读懂当前生效的行为
    expect(wrapper.text()).toContain('snapshot：每个上报周期每个点位只发最新值')
    expect(wrapper.find('[data-strategy="realtime"]').classes()).toContain('active')

    await wrapper.find('[data-strategy="change"]').trigger('click')
    await nextTick()

    // 变化上报必须逐样本：snapshot 会把同周期多次变化折叠成最后一次，事件流水不完整
    expect(mqtt.report_mode).toBe('stream')
    expect(wrapper.find('[data-strategy="change"]').classes()).toContain('active')
    const guide = wrapper.find('[data-guide]')
    expect(guide.exists()).toBe(true)
    expect(guide.text()).toContain('数据压缩')
    expect(guide.text()).toContain('变化才上传')
    expect(guide.text()).toContain('死区为 0 时仅过滤完全相同的值')
    wrapper.unmount()
  })

  it('点击「周期快照」写回 snapshot，且不展开引导', async () => {
    const mqtt = makeMqtt()
    const wrapper = mountTab(mqtt)
    await wrapper.find('[data-strategy="snapshot"]').trigger('click')
    await nextTick()
    expect(mqtt.report_mode).toBe('snapshot')
    expect(wrapper.find('[data-strategy="snapshot"]').classes()).toContain('active')
    expect(wrapper.find('[data-guide]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('变化上报切到周期快照后引导收起', async () => {
    const mqtt = makeMqtt()
    const wrapper = mountTab(mqtt)
    await wrapper.find('[data-strategy="change"]').trigger('click')
    await nextTick()
    expect(wrapper.find('[data-guide]').exists()).toBe(true)

    await wrapper.find('[data-strategy="snapshot"]').trigger('click')
    await nextTick()
    expect(mqtt.report_mode).toBe('snapshot')
    expect(wrapper.find('[data-guide]').exists()).toBe(false)
    expect(wrapper.find('[data-strategy="snapshot"]').classes()).toContain('active')
    wrapper.unmount()
  })

  it('统计上报引导说明聚合窗口需 ≥ 上报周期，并跳转点位页', async () => {
    pushMock.mockClear()
    const wrapper = mountTab(makeMqtt())
    await wrapper.find('[data-strategy="stats"]').trigger('click')
    await nextTick()

    const guide = wrapper.find('[data-guide]')
    expect(guide.text()).toContain('聚合窗口必须 ≥ 上报周期（当前 5 秒）')
    await guide.find('.guide-link').trigger('click')
    expect(pushMock).toHaveBeenCalledWith({ path: '/points' })
    wrapper.unmount()
  })

  it('引导链接跳到数据压缩页签（?tab=data-compression）', async () => {
    pushMock.mockClear()
    const wrapper = mountTab(makeMqtt())
    await wrapper.find('[data-strategy="change"]').trigger('click')
    await wrapper.find('.guide-link').trigger('click')
    expect(pushMock).toHaveBeenCalledWith({ path: '/data', query: { tab: 'data-compression' } })
    wrapper.unmount()
  })

  it('选「实时逐样本」切回 stream 并收起引导', async () => {
    const mqtt = makeMqtt()
    const wrapper = mountTab(mqtt)
    await wrapper.find('[data-strategy="stats"]').trigger('click')
    await nextTick()
    expect(wrapper.find('[data-guide]').exists()).toBe(true)

    await wrapper.find('[data-strategy="realtime"]').trigger('click')
    await nextTick()
    expect(mqtt.report_mode).toBe('stream')
    expect(wrapper.find('[data-guide]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('已保存 snapshot 配置时「周期快照」卡片为选中态，且不展开引导', () => {
    const wrapper = mountTab(makeMqtt({ report_mode: 'snapshot' }))
    expect(wrapper.find('[data-strategy="snapshot"]').classes()).toContain('active')
    expect(wrapper.find('[data-strategy="realtime"]').classes()).not.toContain('active')
    expect(wrapper.find('[data-guide]').exists()).toBe(false)
    wrapper.unmount()
  })

  it('父级重置配置（替换 mqtt 对象）后引导态复位', async () => {
    const wrapper = mountTab(makeMqtt())
    await wrapper.find('[data-strategy="stats"]').trigger('click')
    await nextTick()
    expect(wrapper.find('[data-guide]').exists()).toBe(true)

    // 「放弃修改 / 重新加载」会整体替换 config.mqtt，引导态必须跟着清空
    await wrapper.setProps({ modelValue: makeMqtt({ report_mode: 'stream' }) })
    await nextTick()
    expect(wrapper.find('[data-guide]').exists()).toBe(false)
    expect(wrapper.find('[data-strategy="realtime"]').classes()).toContain('active')
    wrapper.unmount()
  })

  it('MQTT 未启用时提示策略不会生效', () => {
    const wrapper = mountTab(makeMqtt({ enabled: false }))
    expect(wrapper.find('[data-disabled-hint]').text()).toContain('MQTT 未启用')
    // 仍然允许选择（与表单其它字段一致，便于先配好再启用）
    expect(wrapper.find('[data-strategy="stats"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('摘要包含限速与自定义 Payload 等非默认项', () => {
    const wrapper = mountTab(makeMqtt({ compression: 'gzip', resume_rate_limit: 500, payload_template: '{"v":${v}}' }))
    const text = wrapper.text()
    expect(text).toContain('压缩 gzip')
    expect(text).toContain('限速 500 条/秒')
    expect(text).toContain('自定义 Payload')
    wrapper.unmount()
  })

  it('未显式配置的心跳/ACK 在摘要里显示「默认」而非空值', () => {
    // 实际配置文件里这两项可能为 null（走后端默认值），摘要不能出现 "-s" 这种半截文案
    const wrapper = mountTab(makeMqtt({
      keep_alive: null as unknown as number,
      ack_timeout_secs: null as unknown as number,
    }))
    const text = wrapper.text()
    expect(text).toContain('心跳 默认')
    expect(text).toContain('ACK 默认')
    expect(text).not.toContain('-s')
    wrapper.unmount()
  })
})

describe('MqttConfigTab.vue TLS CA 证书', () => {
  it('填写路径后写入 tls.ca_cert_path，且不改动 insecure_skip_verify 原值', async () => {
    const mqtt = makeMqtt()
    const wrapper = mountTab(mqtt)
    await wrapper.find('.el-collapse-item__header').trigger('click')
    await nextTick()

    const input = wrapper.findAll('input').find((i) => i.attributes('placeholder') === '留空 = 使用系统根证书')
    expect(input, 'CA 证书路径输入框缺失').toBeTruthy()
    await input!.setValue('/etc/gateway/ca.pem')

    expect(mqtt.tls).toEqual({ ca_cert_path: '/etc/gateway/ca.pem', insecure_skip_verify: false })
    wrapper.unmount()
  })

  it('清空路径时把 tls 整体置回 null（恢复系统根证书语义）', async () => {
    const mqtt = makeMqtt({ tls: { ca_cert_path: '/etc/gateway/ca.pem', insecure_skip_verify: false } })
    const wrapper = mountTab(mqtt)
    await wrapper.find('.el-collapse-item__header').trigger('click')
    await nextTick()

    const input = wrapper.findAll('input').find((i) => i.attributes('placeholder') === '留空 = 使用系统根证书')
    expect(input).toBeTruthy()
    await input!.setValue('')

    expect(mqtt.tls).toBeNull()
    wrapper.unmount()
  })
})
