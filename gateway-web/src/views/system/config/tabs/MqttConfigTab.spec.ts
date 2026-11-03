// MqttConfigTab.vue 单元测试：基础参数平铺 + 高级参数分组默认折叠。
// 折叠的意义：心跳/ACK/压缩/限速/Payload 模板属于调优项，多数部署用默认值即可，
// 折叠后首屏只留必填项，同时标题上给出当前值摘要，不必展开也能确认配置。
import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import MqttConfigTab from './MqttConfigTab.vue'
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
    for (const label of ['心跳间隔（秒）', 'ACK 超时（秒）', '压缩方式', '压缩阈值（字节）', '续传限速（条/秒）', 'Payload 模板']) {
      expect(text, `高级字段缺失：${label}`).toContain(label)
    }
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
