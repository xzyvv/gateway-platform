/**
 * 大数据集降采样（LTTB 算法：Largest Triangle Three Buckets）
 *
 * 用于 echarts 图表渲染前的前置降采样，避免 >10000 点时卡顿。
 * 纯函数，不依赖 Vue / echarts，便于单元测试与复用。
 */

/**
 * 按需降采样：数据点数超过 `threshold` 时才按 `target` 数量降采样。
 *
 * @param data 原始数据点
 * @param threshold 触发降采样的阈值（数据点数 <= threshold 时直接返回原数据）
 * @param target 降采样目标点数
 */
export function downsampleIfNeeded<T extends { timestamp: number; value: number }>(
  data: T[],
  threshold: number,
  target: number,
): T[] {
  if (data.length <= threshold) return data
  if (target >= data.length) return data
  return lttb(data, target)
}

/**
 * LTTB 降采样实现
 *
 * 算法：将数据分为 (target-2) 个桶，每桶选一个"代表点"，
 * 代表点与相邻桶的代表点构成最大三角形面积，保留视觉特征（峰、谷、趋势）。
 * 首尾两点固定保留。
 */
function lttb<T extends { timestamp: number; value: number }>(data: T[], target: number): T[] {
  const n = data.length
  // Bug P0-B4 (fix): 旧守门 `target >= n || target < 3` 不覆盖 n=1 / n=2 +
  // target=3 的边界——bucketSize = (1-2)/(3-2) = -1，bucketStart=1, bucketEnd=0
  // 导致内层循环不执行但 maxIdx=1 越界到 data[1]=undefined。
  // LTTB 算法要求至少 3 点（首+末+至少一个内部采样点），n<3 直接返回原数据。
  if (n < 3 || target >= n || target < 3) return data
  const bucketSize = (n - 2) / (target - 2)
  const sampled: T[] = []
  sampled.push(data[0]!)
  let prevSelected = data[0]!
  for (let i = 0; i < target - 2; i++) {
    const bucketStart = Math.floor((i) * bucketSize) + 1
    const bucketEnd = Math.floor((i + 1) * bucketSize) + 1
    const nextBucketStart = Math.floor((i + 1) * bucketSize) + 1
    const nextBucketEnd = Math.min(Math.floor((i + 2) * bucketSize) + 1, n)
    // 计算下一桶的平均点
    let avgX = 0, avgY = 0
    let count = 0
    for (let j = nextBucketStart; j < nextBucketEnd; j++) {
      avgX += data[j]!.timestamp
      avgY += data[j]!.value
      count++
    }
    if (count === 0) continue
    avgX /= count
    avgY /= count
    // 在当前桶中选择与 (prev, avg) 构成最大三角形面积的点
    let maxArea = -1
    let maxIdx = bucketStart
    const prevX = prevSelected.timestamp
    const prevY = prevSelected.value
    for (let j = bucketStart; j < bucketEnd && j < n; j++) {
      const point = data[j]!
      const area = Math.abs(
        (prevX - avgX) * (point.value - prevY) - (prevX - point.timestamp) * (avgY - prevY),
      ) * 0.5
      if (area > maxArea) {
        maxArea = area
        maxIdx = j
      }
    }
    prevSelected = data[maxIdx]!
    sampled.push(prevSelected)
  }
  sampled.push(data[n - 1]!)
  return sampled
}