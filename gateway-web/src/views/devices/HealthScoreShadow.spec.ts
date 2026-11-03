// R3：设备影子并入健康评分页后，验证「期望 vs 实测」子表与点位明细抽屉。
// 原 DeviceShadow.spec.ts 随独立页签下线而废弃；能力断言在这里保留。
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

const { mockHealth, mockShadows } = vi.hoisted(() => ({
  mockHealth: vi.fn(),
  mockShadows: vi.fn(),
}))

vi.mock('@/api/deviceOps', async (importOriginal) => {
  // only 替换请求函数，summarizeHealth / qualityLabel 等纯函数用真实实现，
  // 保证本测试验证的是页面展示口径，而不是 mock 出来的假口径
  const actual = await importOriginal<typeof import('@/api/deviceOps')>()
  return { ...actual, listHealthView: mockHealth, listShadowsView: mockShadows }
})

import HealthScore from './HealthScore.vue'

function shadow(overrides: Record<string, unknown> = {}) {
  return {
    deviceId: 'd1',
    deviceName: '电表1',
    protocol: 'modbus',
    expectedIntervalMs: 5_000,
    lastValueHint: null,
    lastQuality: 'good',
    lastUpdateAt: Date.now() - 3_000,
    points: [
      { id: 'p1', lastValue: 12.5, quality: 'good' },
      { id: 'p2', lastValue: null, quality: 'bad' },
    ],
    ...overrides,
  }
}

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
})

async function mountPage() {
  const wrapper = mount(HealthScore, { attachTo: document.body })
  wrappers.push(wrapper)
  await flushPromises()
  await nextTick()
  return wrapper
}

describe('HealthScore.vue — 期望 vs 实测（设备影子）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockHealth.mockResolvedValue([])
    mockShadows.mockResolvedValue([shadow()])
  })

  it('渲染期望周期、相对期望与点位质量分布', async () => {
    const wrapper = await mountPage()

    const text = wrapper.text()
    expect(text).toContain('期望 vs 实测')
    expect(text).toContain('电表1')
    expect(text).toContain('modbus')
    // 5000ms 按秒展示，不直接铺裸毫秒数
    expect(text).toContain('5 秒')
    expect(text).toContain('在期望内')
    expect(text).toContain('良好')
    expect(text).toContain('好 1 / 不确定 0 / 坏 1')
  })

  it('lastUpdateAt=0（从未成功采集）显示「无上报」而不是伪装成正常', async () => {
    mockShadows.mockResolvedValue([shadow({ lastUpdateAt: 0, lastQuality: 'uncertain' })])
    const wrapper = await mountPage()

    const text = wrapper.text()
    expect(text).toContain('无上报')
    expect(text).toContain('不确定')
    expect(text).not.toContain('在期望内')
  })

  it('列表为空时展示快照空态说明', async () => {
    mockShadows.mockResolvedValue([])
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('暂无快照数据，设备首次上报采集值后自动生成')
  })

  it('查看点位打开明细抽屉：null 值显示占位符，质量标签跟随点位', async () => {
    const wrapper = await mountPage()

    const btn = wrapper.findAll('button').find((b) => b.text().includes('查看点位'))
    expect(btn, '未找到「查看点位」按钮').toBeTruthy()
    await btn!.trigger('click')
    await nextTick()
    await flushPromises()

    const drawerText = document.body.textContent ?? ''
    expect(drawerText).toContain('点位明细')
    expect(drawerText).toContain('12.5')
    // 无采集值显示「—」，不显示 0 / null
    expect(drawerText).toContain('—')
    expect(drawerText).toContain('坏值')
  })

  it('刷新评分同时重新拉取评分与快照，避免两个时点数据并排', async () => {
    const wrapper = await mountPage()
    expect(mockHealth).toHaveBeenCalledTimes(1)
    expect(mockShadows).toHaveBeenCalledTimes(1)

    const refreshBtn = wrapper.findAll('button').find((b) => b.text().includes('刷新评分'))
    expect(refreshBtn, '未找到「刷新评分」按钮').toBeTruthy()
    await refreshBtn!.trigger('click')
    await flushPromises()

    expect(mockHealth).toHaveBeenCalledTimes(2)
    expect(mockShadows).toHaveBeenCalledTimes(2)
  })
})
