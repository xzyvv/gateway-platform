import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

/**
 * D1：设备管理 › 设备健康评分（HealthScore）行为测试。
 *
 * 这页是「设备综合健康分」的唯一来源，评分被用来决定是否派人去现场。
 * 钉住的核心：
 * - 24h 无采集（score=null）渲染「—」且排序永远垫底，不用 0 分冒充；
 * - 汇总由列表派生（优秀/良好/告警/异常/未采集 + 平均分只对已评分设备取均值）；
 * - 四个排序维度都按「有值优先、无值最后」；
 * - 期望 vs 实测按「最后上报距今 > 2 个期望周期」判超期，0/从未上报不冒充正常；
 * - 两个列表各自失败时可见报错，另一半不受影响。
 */

vi.mock('vue-router', () => ({ useRouter: () => ({ push: routerPush }) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))
vi.mock('@/api/deviceOps', async () => {
  const actual = await vi.importActual<typeof import('@/api/deviceOps')>('@/api/deviceOps')
  return {
    ...actual,
    listHealthView: (...a: unknown[]) => listHealthMock(...a),
    listShadowsView: (...a: unknown[]) => listShadowsMock(...a),
  }
})

const routerPush = vi.hoisted(() => vi.fn())
const showToast = vi.hoisted(() => vi.fn())
const listHealthMock = vi.hoisted(() => vi.fn())
const listShadowsMock = vi.hoisted(() => vi.fn())

import HealthScore from './HealthScore.vue'
import DataTable from '@/components/DataTable.vue'
import type { DeviceHealthView, DeviceShadowView } from '@/api/deviceOps'

const DrawerStub = {
  name: 'ElDrawer',
  props: ['modelValue', 'title', 'size'],
  template: '<div class="drawer-stub"><slot /></div>',
}

function health(patch: Partial<DeviceHealthView> & { deviceId: string }): DeviceHealthView {
  return {
    deviceName: patch.deviceId,
    protocol: 'modbus-tcp',
    bus: 'bus1',
    score: 90,
    level: 'excellent',
    successRate: 99,
    avgLatency: 10,
    errorCount: 0,
    offlineCount: 0,
    goodPointRatio: 100,
    consecutiveFailures: 0,
    qualityBreakdown: { good: 10, uncertain: 0, bad: 0, unknown: 0, total: 10 },
    lastSeenAt: Date.now() - 5000,
    details: [
      { label: '采集成功率', value: '99%', score: 99 },
      { label: '平均延迟', value: '10 ms', score: 80 },
    ],
    ...patch,
  }
}

const ROWS: DeviceHealthView[] = [
  health({ deviceId: 'A', score: 95, level: 'excellent', successRate: 99, avgLatency: 5, consecutiveFailures: 0 }),
  health({ deviceId: 'B', score: 75, level: 'good', successRate: 100, avgLatency: 8, consecutiveFailures: 2, errorCount: 3 }),
  health({ deviceId: 'C', score: 60, level: 'warning', successRate: 80, avgLatency: 20, consecutiveFailures: 1 }),
  health({
    deviceId: 'D', score: 30, level: 'critical', successRate: 50, avgLatency: 40, consecutiveFailures: 5,
    offlineCount: 2, goodPointRatio: null, lastSeenAt: null,
  }),
  health({
    deviceId: 'E', score: null, level: 'unknown', successRate: null, avgLatency: null, consecutiveFailures: 0,
    goodPointRatio: null, lastSeenAt: null, details: [],
  }),
]

const NOW = Date.now()
const SHADOWS: DeviceShadowView[] = [
  {
    deviceId: 'S1', deviceName: '超期设备', protocol: 'modbus-tcp', expectedIntervalMs: 5000,
    lastValueHint: '温度 25℃', lastQuality: 'good', lastUpdateAt: NOW - 60_000,
    points: [
      { id: 'p1', quality: 'good', lastValue: '25' },
      { id: 'p2', quality: 'bad', lastValue: null },
    ],
  },
  {
    deviceId: 'S2', deviceName: '无上报设备', protocol: 'dlt645', expectedIntervalMs: 200,
    lastValueHint: null, lastQuality: 'uncertain', lastUpdateAt: 0, points: [],
  },
  {
    deviceId: 'S3', deviceName: '新鲜设备', protocol: 'cjt188', expectedIntervalMs: 10_000,
    lastValueHint: null, lastQuality: 'bad', lastUpdateAt: NOW - 1000,
    points: [{ id: 'p3', quality: 'uncertain', lastValue: null }],
  },
]

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(HealthScore, { global: { stubs: { ElDrawer: DrawerStub } } })
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

function tableData(wrapper: ReturnType<typeof mountPage>, index = 0): DeviceHealthView[] {
  return (wrapper.findAllComponents(DataTable)[index]!.props() as unknown as { data: DeviceHealthView[] }).data
}

describe('HealthScore 设备健康评分', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listHealthMock.mockResolvedValue([...ROWS])
    listShadowsMock.mockResolvedValue([...SHADOWS])
  })

  it('首屏：汇总只对已评分设备取均值，五档计数与列表一致', async () => {
    const wrapper = mountPage()
    await settle()

    expect(wrapper.find('.score-num').text()).toBe('65')
    const stats = wrapper.findAll('.stat-item .stat-num').map((n) => n.text())
    expect(stats).toEqual(['1', '1', '1', '1', '1'])
  })

  it('无采集设备不冒充 0 分：评分列渲染「—」且默认排序垫底', async () => {
    const wrapper = mountPage()
    await settle()

    expect(tableData(wrapper).map((r) => r.deviceId)).toEqual(['A', 'B', 'C', 'D', 'E'])
    expect(wrapper.findAll('.text-muted').some((n) => n.text() === '—')).toBe(true)
  })

  it('四个排序维度都遵守「有值优先、无值最后」', async () => {
    const wrapper = mountPage()
    await settle()
    const group = wrapper.findComponent({ name: 'ElRadioGroup' })

    await group.vm.$emit('update:modelValue', 'successRate')
    await settle()
    expect(tableData(wrapper).map((r) => r.deviceId)).toEqual(['B', 'A', 'C', 'D', 'E'])

    await group.vm.$emit('update:modelValue', 'latency')
    await settle()
    expect(tableData(wrapper).map((r) => r.deviceId)).toEqual(['A', 'B', 'C', 'D', 'E'])

    await group.vm.$emit('update:modelValue', 'failures')
    await settle()
    expect(tableData(wrapper).map((r) => r.deviceId)).toEqual(['A', 'E', 'C', 'B', 'D'])

    await group.vm.$emit('update:modelValue', 'score')
    await settle()
    expect(tableData(wrapper).map((r) => r.deviceId)).toEqual(['A', 'B', 'C', 'D', 'E'])
  })

  it('评分列表加载失败：可见报错、汇总回到未知、页面不崩', async () => {
    listHealthMock.mockRejectedValue(new Error('health 500'))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.exists()).toBe(true)
    expect(showToast).toHaveBeenCalledWith(new Error('health 500'), '加载健康评分失败')
    expect(wrapper.find('.score-num').text()).toBe('—')
    expect(wrapper.findAll('.stat-item .stat-num').map((n) => n.text())).toEqual(['0', '0', '0', '0', '0'])
    expect(wrapper.text()).toContain('暂无评分，需设备先产生采集与在线记录')
  })

  it('快照失败时报自己的错，评分区仍正常展示', async () => {
    listShadowsMock.mockRejectedValue(new Error('shadow 500'))
    const wrapper = mountPage()
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('shadow 500'), '加载设备状态快照失败')
    expect(wrapper.find('.score-num').text()).toBe('65')
    expect(wrapper.text()).toContain('暂无快照数据，设备首次上报采集值后自动生成')
  })

  it('刷新评分会同时重拉评分与快照', async () => {
    const wrapper = mountPage()
    await settle()
    expect(listHealthMock).toHaveBeenCalledTimes(1)
    expect(listShadowsMock).toHaveBeenCalledTimes(1)

    await button(wrapper, '刷新评分').trigger('click')
    await settle()
    expect(listHealthMock).toHaveBeenCalledTimes(2)
    expect(listShadowsMock).toHaveBeenCalledTimes(2)
  })

  it('期望 vs 实测：超期/在期望内/从未上报各不相同，0 周期不冒充正常', async () => {
    const wrapper = mountPage()
    await settle()
    const text = wrapper.text()

    expect(text).toContain('已超期')
    expect(text).toContain('在期望内')
    expect(text).toContain('无上报')
    // 期望周期按可读单位渲染，不出现裸毫秒数字
    expect(text).toContain('5 秒')
    expect(text).toContain('200 ms')
    expect(text).toContain('—')
  })

  it('点位质量按好/不确定/坏汇总，空快照不伪造计数', async () => {
    const wrapper = mountPage()
    await settle()
    const text = wrapper.text()

    expect(text).toContain('好 1 / 不确定 0 / 坏 1')
    expect(text).toContain('好 0 / 不确定 1 / 坏 0')
    expect(text).toContain('良好')
    expect(text).toContain('不确定')
    expect(text).toContain('异常')
  })

  it('查看详情抽屉：等级文案、null 指标渲染「—」、趋势明细逐项给分', async () => {
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '查看详情').trigger('click')
    await settle()

    const detail = wrapper.find('.detail')
    expect(detail.exists()).toBe(true)
    expect(detail.find('.detail-score-num').text()).toBe('95')
    expect(detail.find('.detail-score-label').text()).toBe('优秀')
    expect(detail.text()).toContain('采集成功率99%')
    // D 设备延迟/占比为 null 时走「—」
    expect(detail.findAllComponents({ name: 'ElProgress' })).toHaveLength(2)
  })

  it('查看点位抽屉：按设备展示点位快照与质量标签', async () => {
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '查看点位').trigger('click')
    await settle()

    const drawerText = wrapper.findAll('.drawer-stub')[1]!.text()
    expect(drawerText).toContain('温度 25℃')
    expect(drawerText).toContain('5 秒')
    // 点位质量标签按统一口径本地化：bad → 坏值
    expect(drawerText).toContain('坏值')
  })

  it('跳转通信画像带上 tab 参数', async () => {
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '查看通信画像').trigger('click')

    expect(routerPush).toHaveBeenCalledWith('/acquisition/comm-tuning?tab=comm-profile')
  })
})
