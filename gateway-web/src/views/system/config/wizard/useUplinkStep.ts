/**
 * 部署向导 · 第 4 步（上行转发）的状态与真实接口调用。
 *
 * 本步只写 MQTT 主通道（网关默认北向），其余北向通道在「运行参数」页按分节配置。
 * 这里持有上行表单与读取/保存逻辑，页面只把状态透传给子组件渲染。
 *
 * 典型应用场景：部署向导的第 4 步（配置上行转发）。
 */
import { ref } from 'vue'
import { ElMessage } from 'element-plus'
import { getConfig, postConfig } from '@/api/config'
import type { UplinkFormModel } from './types'

export function useUplinkStep() {
  /** 上行转发表单值 */
  const uplinkForm = ref<UplinkFormModel>({ enabled: true, broker: '', topic: '', publishInterval: 5, qos: 1 })

  /** 读取网关已有的 MQTT 配置填表；读取失败不阻断，用户可在步骤 4 手填 */
  async function loadUplink() {
    try {
      const cfg = await getConfig()
      const m = cfg.mqtt
      if (m) {
        uplinkForm.value = {
          enabled: m.enabled ?? true,
          broker: m.broker ?? '',
          topic: m.topic ?? '',
          publishInterval: m.publish_interval ?? 5,
          qos: m.qos ?? 1,
        }
      }
    } catch {
      // 上行配置读取失败不阻断，用户可在步骤 4 手填
    }
  }

  /** 保存 MQTT 上行分节；成功返回 true */
  async function saveUplink(): Promise<boolean> {
    const broker = uplinkForm.value.broker.trim()
    const topic = uplinkForm.value.topic.trim()
    if (uplinkForm.value.enabled && !broker) {
      ElMessage.warning('请填写 MQTT Broker 地址')
      return false
    }
    if (uplinkForm.value.enabled && !topic) {
      ElMessage.warning('请填写上报主题')
      return false
    }
    try {
      const cfg = await getConfig()
      cfg.mqtt = {
        ...cfg.mqtt,
        enabled: uplinkForm.value.enabled,
        broker,
        topic,
        publish_interval: uplinkForm.value.publishInterval,
        qos: uplinkForm.value.qos,
      }
      await postConfig(cfg)
      ElMessage.success('上行转发配置已保存')
      return true
    } catch (e) {
      ElMessage.error(e instanceof Error && e.message ? e.message : '保存上行配置失败')
      return false
    }
  }

  return {
    uplinkForm,
    loadUplink,
    saveUplink,
  }
}
