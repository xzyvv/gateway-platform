/**
 * 采集退避状态的统一读取与口径（E3：退避判断唯一实现）
 *
 * 背景：`GET /api/devices/backoff` 被三处使用——退避监控页（统计三类状态）、
 * 趋势页（点位候选 + 兜底）、设备列表（按设备聚合最后成功时间）。
 * 三处各自 `filter().length`、各自从引退避列表里推算设备归属，口径容易漂移。
 *
 * 本模块提供：
 *   - `summarizeBackoff`：正常 / 失败未退避 / 退避中三类计数（页面唯一口径）；
 *   - `lastGoodByDevice`：设备最后成功时间（含空 device_id 的前缀反解）；
 *   - `fetchPointBackoffOnce`：同刻只发一次请求，多个页面共享结果；
 *   - `usePointBackoff`：组件内状态（loading / error / 列表）。
 */
import { computed, ref } from 'vue'
import { listPointBackoff, type PointBackoff } from '@/api/dataQuality'
import { extractErrorMessage } from '@/utils/errorHandler'

export interface BackoffStats {
  total: number
  /** 连续失败 >=3 次、已拉长采集间隔 */
  backingOff: number
  /** 有连续失败但还没触发退避 */
  failing: number
  /** 无连续失败 */
  normal: number
}

/** 退避页面三类计数的唯一口径 */
export function summarizeBackoff(points: PointBackoff[]): BackoffStats {
  let backingOff = 0
  let failing = 0
  let normal = 0
  for (const p of points) {
    if (p.backing_off) backingOff++
    else if (p.consecutive_failures > 0) failing++
    else normal++
  }
  return { total: points.length, backingOff, failing, normal }
}

/**
 * 退避记录 → 设备 ID。
 *
 * 后端退避快照里 `device_id` 可能为空（模板展开点位的归属未写入），
 * 此时用 `${设备ID}_` 前缀从 point_id 反解；反解不出来就返回空串，不猜。
 */
export function resolveBackoffDeviceId(
  b: PointBackoff,
  knownDeviceIds: Iterable<string>,
): string {
  if (b.device_id) return b.device_id
  for (const id of knownDeviceIds) {
    if (b.point_id.startsWith(`${id}_`)) return id
  }
  return ''
}

/** 按设备聚合最后成功时间（毫秒，0 = 从未成功） */
export function lastGoodByDevice(
  points: PointBackoff[],
  knownDeviceIds: Iterable<string>,
): Record<string, number> {
  const map: Record<string, number> = {}
  for (const b of points) {
    if (!b.last_good_ts) continue
    const devId = resolveBackoffDeviceId(b, knownDeviceIds)
    if (!devId) continue
    if (b.last_good_ts > (map[devId] ?? 0)) map[devId] = b.last_good_ts
  }
  return map
}

/** 同刻并发调用共享同一个请求；请求结束后允许下一次重新拉取 */
let inflight: Promise<PointBackoff[]> | null = null
export function fetchPointBackoffOnce(): Promise<PointBackoff[]> {
  if (!inflight) {
    inflight = listPointBackoff().finally(() => {
      inflight = null
    })
  }
  return inflight
}

export function usePointBackoff() {
  const points = ref<PointBackoff[]>([])
  const loading = ref(false)
  const error = ref('')

  async function load(): Promise<void> {
    loading.value = true
    error.value = ''
    try {
      points.value = await fetchPointBackoffOnce()
    } catch (e) {
      // 退避接口失败不应阻塞页面主流程（趋势页仍可手输、设备列表仍可管理）
      error.value = extractErrorMessage(e, '退避状态读取失败')
    } finally {
      loading.value = false
    }
  }

  const stats = computed(() => summarizeBackoff(points.value))

  return { points, loading, error, stats, load }
}
