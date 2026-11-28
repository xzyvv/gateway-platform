import { describe, it, expect, beforeEach } from 'vitest'
import { computed, nextTick } from 'vue'
import { recordRead, getDeviceStats, clearDeviceStats } from './deviceComms'

/**
 * 回归测试：`STATS` 必须是响应式的。
 *
 * 裸 Map 下读写在任何依赖之外，调用方的 computed 首次求值后被永久缓存：
 * 设备列表中"新设备"永远不会出现在 commsStats 里 → 通信质量列恒显示"无数据"，
 * 必须离开页面再回来才刷新（useDevices.ts 的 commsStats）。
 */
describe('deviceComms 响应式统计', () => {
  beforeEach(() => {
    clearDeviceStats()
  })

  it('新设备首次记录后，遍历型 computed 能重新求值', async () => {
    const deviceIds = computed(() => getDeviceStats().map((s) => s.deviceId))
    expect(deviceIds.value).toEqual([])

    recordRead('dev-a', true, 10)
    await nextTick()

    expect(deviceIds.value).toEqual(['dev-a'])
  })

  it('已有设备再次记录后，读取字段的 computed 能重新求值', async () => {
    recordRead('dev-a', true, 10)
    const totalReads = computed(() => getDeviceStats().reduce((n, s) => n + s.totalReads, 0))
    expect(totalReads.value).toBe(1)

    recordRead('dev-a', true, 20)
    await nextTick()

    expect(totalReads.value).toBe(2)
  })

  it('清空统计后 computed 也能感知', async () => {
    recordRead('dev-a', true, 10)
    const count = computed(() => getDeviceStats().length)
    expect(count.value).toBe(1)

    clearDeviceStats()
    await nextTick()

    expect(count.value).toBe(0)
  })
})
