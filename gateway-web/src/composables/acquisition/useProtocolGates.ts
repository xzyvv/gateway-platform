/**
 * 设备 → 协议能力门控（共享）
 *
 * 「选中这台设备后，当前协议支持哪些能力」是调试页的通用问题：点位读写、
 * 读取性能…都要按能力位决定按钮/面板是否出现，而不是按协议名硬编码。
 * 此前每页各写一遍 `protocolCaps[protocol]` 加 6 个 `supports_*` computed，
 * 最容易写歪的就是「能力未知」的判定口径，故收敛到这里。
 *
 * 门控口径：元数据可用时严格按声明渲染；不可用（未加载 / 加载失败 / 未知协议）
 * 时**按不支持处理**。「能力未知」不等于「能力具备」——写入等副作用入口若因
 * 元数据缺失而放行，用户会看到一个必然返回 `Unsupported` 的按钮，误判为设备故障。
 * 这与从站扫描页的 `caps ? caps.supports_scan_slaves : false` 是同一口径。
 *
 * 用法（devices / selectedDeviceId 由调用方持有，本组合式函数不做数据加载）：
 * ```ts
 * const { selectedProtocol, capsKnown, supportsWrite } = useProtocolGates(devices, selectedDevice)
 * ```
 * 协议元数据经 `useProtocolCaps()` 获取，挂载时自动加载一次。
 */
import { computed, type Ref } from 'vue'
import { useProtocolCaps } from './useProtocolCaps'
import type { DeviceConfig } from '@/api/types'

export function useProtocolGates(devices: Ref<DeviceConfig[]>, selectedDeviceId: Ref<string>) {
  const { capsOf } = useProtocolCaps()

  /** 所选设备对应的协议名；未选设备 / 设备已删除时为空串 */
  const selectedProtocol = computed(
    () => devices.value.find((d) => d.id === selectedDeviceId.value)?.protocol || '',
  )
  /** 所选协议的能力声明；元数据缺失（未加载 / 未知协议 / 加载失败）时为 null */
  const selectedCaps = computed(() => capsOf(selectedProtocol.value))

  return {
    selectedProtocol,
    selectedCaps,
    /** 能力元数据是否已知（false 时下方各 supports* 一律为 false） */
    capsKnown: computed(() => selectedCaps.value !== null),
    supportsWrite: computed(() => selectedCaps.value?.supports_write ?? false),
    supportsBatchRead: computed(() => selectedCaps.value?.supports_batch_read ?? false),
    supportsScanSlaves: computed(() => selectedCaps.value?.supports_scan_slaves ?? false),
    supportsDiag: computed(() => selectedCaps.value?.supports_diag ?? false),
  }
}
