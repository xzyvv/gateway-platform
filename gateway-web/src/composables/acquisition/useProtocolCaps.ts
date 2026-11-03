/**
 * 协议能力门控（共享）：`GET /api/protocols` → 协议名 → 能力位映射
 *
 * 调试工具页（从站扫描 / 寄存器扫描 / 参数探测）都按能力位决定按钮可用性，
 * 而不是按协议名硬编码，后端注册表新增能力即为前端新增入口。
 *
 * 能力元数据缺失（加载失败）时一律按「不支持」处理：能力未知不等于具备，
 * 否则用户会看到一个「点了必然失败」的按钮，误判为设备/网关故障。
 */
import { onMounted, ref } from 'vue'
import { listProtocolCapabilities } from '@/api/protocols'
import type { ProtocolCapabilities } from '@/api/generated/ProtocolCapabilities'

export function useProtocolCaps() {
  const caps = ref<Record<string, ProtocolCapabilities>>({})
  /** 加载流程已结束（成功或失败），用于区分「加载中」与「确实不支持」 */
  const loaded = ref(false)
  const failed = ref(false)

  async function load() {
    try {
      const list = await listProtocolCapabilities()
      const map: Record<string, ProtocolCapabilities> = {}
      for (const e of list) map[e.name] = e.capabilities
      caps.value = map
    } catch {
      failed.value = true
    } finally {
      loaded.value = true
    }
  }

  onMounted(load)

  /** 取某协议的能力声明；未知协议 / 元数据未加载时返回 null（按不支持处理） */
  function capsOf(protocol: string): ProtocolCapabilities | null {
    return protocol ? caps.value[protocol] ?? null : null
  }

  return { caps, loaded, failed, load, capsOf }
}
