// MqttConfigTab.vue 单元测试：基础参数平铺 + 高级参数分组默认折叠。
// 折叠的意义：心跳/ACK/压缩/限速/Payload 模板属于调优项，多数部署用默认值即可，
// 折叠后首屏只留必填项，同时标题上给出当前值摘要，不必展开也能确认配置。
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import MqttConfigTab from './MqttConfigTab.vue'
import { ElSelect } from 'element-plus'
import type { MqttConfig } from '@/api/types'

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
    for (const label of ['启用 MQTT', 'Broker 地址', 'Topic', 'QoS 等级', '上报周期', '用户名', '密码']) {
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
    expect(wrapper.find('.backoff-hint').text()).toContain('1 / 1 / 2 / 3 / 5 / 10')

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
