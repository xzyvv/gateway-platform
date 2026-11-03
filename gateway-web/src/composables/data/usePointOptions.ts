/**
 * 统一点位下拉选项来源
 *
 * 背景：导出格式 / 数据插值 / 基线对比 / 历史数据 / 示波器 / 抖动分析这些页面的
 * 「点位」下拉，此前一律只取 `GET /api/devices/point-quality`（运行时质量快照）。
 * 该接口在没有运行中的采集器时返回空数组——而这恰恰是新设备刚接入、总线还没跑起来
 * 的常态，于是用户看到一个空下拉，功能整体不可用，且没有任何提示。
 *
 * 这里改为「配置点位 + 运行时快照」双源合并：
 * - `GET /api/config` 的 `points` 是权威配置（逐点模式），一定有数据；
 * - `point-quality` 补充运行时信息（当前值、质量、模板展开的运行时点位）。
 *
 * 合并后下拉不再为空；点位的当前值/质量在无采集器时为 null，各页面按「无值」展示。
 */
import { ref } from 'vue'
import { getConfig } from '@/api/config'
import { type PointQuality } from '@/api/dataQuality'
import { fetchPointQualities } from '@/composables/data/usePointQuality'

export interface PointOption {
  /** 点位 ID（唯一） */
  id: string
  /** 点位中文名，无名时回退到 ID */
  name: string
  /** 所属设备 ID */
  deviceId: string
  /** 所属设备名 */
  deviceName: string
  /** 最近一次采集值；无采集器或从未成功采集时为 null */
  value: number | null
  /** 数据质量；无运行时快照时为 null */
  quality: PointQuality['quality'] | null
}

export function usePointOptions() {
  const pointOptions = ref<PointOption[]>([])
  const loading = ref(false)

  async function load() {
    loading.value = true
    try {
      // 配置与运行时快照并行；任一失败都不阻塞另一路
      const [cfgRes, qualityRes] = await Promise.allSettled([
        getConfig(),
        fetchPointQualities(),
      ])

      const cfg = cfgRes.status === 'fulfilled' ? cfgRes.value : null
      const qualities = qualityRes.status === 'fulfilled' ? qualityRes.value : []

      const deviceNames = new Map<string, string>()
      for (const d of cfg?.devices ?? []) deviceNames.set(d.id, d.name)

      const merged = new Map<string, PointOption>()
      // 先落运行时快照：它带当前值与质量，且含模板展开的运行时点位
      for (const q of qualities) {
        merged.set(q.point_id, {
          id: q.point_id,
          name: q.point_name || q.point_id,
          deviceId: q.device_id,
          deviceName: q.device_name || deviceNames.get(q.device_id) || '-',
          value: q.value,
          quality: q.quality,
        })
      }
      // 再补配置点位：运行时没采到数的点位也要能被选到
      for (const p of cfg?.points ?? []) {
        if (merged.has(p.id)) continue
        merged.set(p.id, {
          id: p.id,
          name: p.name || p.id,
          deviceId: p.device_id ?? '',
          deviceName: deviceNames.get(p.device_id ?? '') || '-',
          value: null,
          quality: null,
        })
      }

      pointOptions.value = [...merged.values()]
    } finally {
      loading.value = false
    }
  }

  return { pointOptions, loading, reload: load }
}
