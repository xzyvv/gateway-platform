/**
 * 点位质量快照的单一取数入口
 *
 * 背景：GET /api/devices/point-quality 此前被 8 处直接调用（数据质量概览、
 * 波形观测、基线对比、通信画像、点位下拉合并、设备清单、链路追踪、历史数据），
 * 各自重复写「拉列表 → 建 point_id→value map」这类派生逻辑。
 * 现在取数与派生统一在本模块，调用方只消费 map。
 *
 * 刻意不做缓存：波形观测每 500ms 采一轮、数据质量页有手动刷新按钮，
 * 任何缓存都会让「刷新」拿到旧值。
 */
import { ref, computed } from 'vue'
import { listPointQuality, type PointQuality } from '@/api/dataQuality'

/**
 * 拉一份全量点位质量快照（全站唯一调用 listPointQuality 的地方）。
 *
 * 需要「失败不阻塞主流程」的地方（设备清单、链路追踪兜底）用这个，
 * 需要派生 map 的地方用 usePointQuality。
 */
export function fetchPointQualities(): Promise<PointQuality[]> {
  return listPointQuality()
}

/** point_id → 最近一次采集值 */
export function pointValueMap(list: PointQuality[]): Map<string, number | null> {
  return new Map((list ?? []).map((q) => [q.point_id, q.value]))
}

/** device_id → 该设备下连续失败点位数之和 */
export function deviceFailureMap(list: PointQuality[]): Map<string, number> {
  const m = new Map<string, number>()
  for (const q of list ?? []) {
    m.set(q.device_id, (m.get(q.device_id) ?? 0) + q.consecutive_failures)
  }
  return m
}

export function usePointQuality() {
  const qualities = ref<PointQuality[]>([])
  const loading = ref(false)
  const error = ref<unknown>(null)

  /** 拉取一次全量快照；失败时抛出，由调用方决定提示或降级 */
  async function load(): Promise<PointQuality[]> {
    loading.value = true
    error.value = null
    try {
      const list = await fetchPointQualities()
      qualities.value = list
      return list
    } catch (e) {
      error.value = e
      throw e
    } finally {
      loading.value = false
    }
  }

  const valueByPoint = computed(() => pointValueMap(qualities.value))
  const failuresByDevice = computed(() => deviceFailureMap(qualities.value))

  return { qualities, loading, error, load, valueByPoint, failuresByDevice }
}
