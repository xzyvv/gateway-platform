import type { DeviceConfig, PointConfig } from '@/api/types'

/**
 * 判断扫描发现的地址是否已存在于设备列表中
 * @param row 扫描结果行（含 address 字段）
 * @param devices 现有设备列表
 * @param bus 当前扫描的总线名
 */
export function isExistingDevice(
  row: { address?: unknown },
  devices: DeviceConfig[],
  bus: string,
): boolean {
  const addr = String(row.address ?? '')
  return devices.some(d => String(d.address ?? '') === addr && d.bus === bus)
}

/**
 * 根据复制策略过滤源点位，返回需要新建的点位列表
 * @param sourcePoints 源设备点位
 * @param targetPoints 目标设备现有点位
 * @param mode 'skip' 跳过同名；'overwrite' 保留全部（覆盖语义由后端处理）
 */
export function filterPointsForCopy(
  sourcePoints: PointConfig[],
  targetPoints: PointConfig[],
  mode: 'skip' | 'overwrite',
): { toCreate: PointConfig[]; skipped: number } {
  if (mode === 'overwrite') {
    return { toCreate: sourcePoints, skipped: 0 }
  }
  const targetIds = new Set(targetPoints.map(p => p.id))
  const toCreate: PointConfig[] = []
  let skipped = 0
  for (const p of sourcePoints) {
    if (targetIds.has(p.id)) {
      skipped++
    } else {
      toCreate.push(p)
    }
  }
  return { toCreate, skipped }
}

/**
 * 将源点位映射到目标设备（设置 device_id）
 */
export function mapPointToTarget(point: PointConfig, targetDeviceId: string): PointConfig {
  return { ...point, device_id: targetDeviceId }
}
