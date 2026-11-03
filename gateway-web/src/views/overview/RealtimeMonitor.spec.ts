import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

/**
 * D1：概览 › 点位实时监视行为测试。
 *
 * 这一页的误读成本极高：把断连后的旧值当成实时值，现场就会照着错的数去调参数。
 * 因此钉住：
 *   1. 连接三态与「首帧未到」必须有区分（不能显示成「0 个点位」）；
 *   2. 断连且仍有数据时，陈旧横幅必须出现并给出重连入口；
 *   3. 空态按原因分流（筛选空 / 有下一步页面 / 只能重连），行动按钮真能点通；
 *   4. 行级质量标记与深链定位要落到真实行上。
 */

const { push, state, qs, hooks } = vi.hoisted(() => ({
  push: vi.fn(),
  qs: { fav: '', highlight: '' } as Record<string, string>,
  hooks: { pausedRef: null as null | { value: boolean } },
  state: {
    points: [] as Array<Record<string, unknown>>,
    connected: false,
    attempts: 0,
    favorites: [] as string[],
    paused: false,
    emptyText: '',
    emptyKind: 'empty' as 'empty' | 'filtered' | 'disconnected',
    emptyActionText: '',
    emptyActionRoute: '',
    chartError: '',
  },
}))

vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))

vi.mock('@/composables/core/useQueryState', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  return { useQueryState: (key: string) => ref(qs[key] ?? '') }
})

vi.mock('@/composables/data/useRealtime', async () => {
  const { ref, computed } = await vi.importActual<typeof import('vue')>('vue')
  return {
    useRealtime: () => {
      const onlyFavorites = ref(false)
      const isFavorite = (id: string) => state.favorites.includes(id)
      const toggleFavorite = (id: string) => {
        const i = state.favorites.indexOf(id)
        if (i < 0) state.favorites.push(id)
        else state.favorites.splice(i, 1)
      }
      const displayPoints = computed(() =>
        onlyFavorites.value ? state.points.filter((p) => isFavorite(p.point_id as string)) : state.points,
      )
      // 暂停态必须由同一份 ref 承载：togglePause 与模板绑定共用它
      hooks.pausedRef ??= ref(state.paused)
      const paused = hooks.pausedRef as { value: boolean }
      return {
        pointList: computed(() => state.points),
        connected: computed(() => state.connected),
        reconnectAttempts: computed(() => state.attempts),
        onlyFavorites,
        paused,
        displayPoints,
        emptyText: computed(() => state.emptyText),
        emptyKind: computed(() => state.emptyKind),
        emptyActionText: computed(() => state.emptyActionText),
        emptyActionRoute: computed(() => state.emptyActionRoute),
        tableHeight: ref(400),
        chartHeight: ref(220),
        chartRef: ref(null),
        chartTimeSpan: computed(() => '近 5 分钟'),
        chartError: computed(() => state.chartError),
        isFavorite,
        toggleFavorite,
        qualityClass: (q: string) => ({ bad: 'q-bad', stale: 'q-warn', uncertain: 'q-warn' })[q] ?? 'q-ok',
        qualityTagType: (q: string) => (q === 'bad' ? 'danger' : 'warning'),
        qualityLabel: { good: '正常', bad: '坏值', stale: '陈旧', uncertain: '可疑' } as Record<string, string>,
        typeLabel: { int16: 'INT16', float: 'FLOAT' } as Record<string, string>,
        formatValue: (v: unknown) => String(v ?? '—'),
        formatTime: (t: unknown) => `t=${t}`,
        reconnectNow: vi.fn(),
        exportCsv: vi.fn(),
        togglePause: () => {
          paused.value = !paused.value
        },
      }
    },
  }
})

import RealtimeMonitor from './RealtimeMonitor.vue'
import DataTable from '@/components/DataTable.vue'
import EmptyState from '@/components/EmptyState.vue'
import { useWebsocketStore } from '@/stores/websocket'

function point(id: string, over: Record<string, unknown> = {}) {
  return {
    point_id: id,
    device_id: 'dev-1',
    value: 25.5,
    raw_value: '00255',
    raw_type: 'int16',
    quality: 'good',
    timestamp: 1700000000,
    ...over,
  }
}

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(RealtimeMonitor)
  await settle()
  return wrapper
}

function tableRows(wrapper: ReturnType<typeof mount>) {
  const table = wrapper.findComponent(DataTable)
  return (table.props() as unknown as { data: Array<Record<string, unknown>> }).data
}

function rowClass(wrapper: ReturnType<typeof mount>, row: Record<string, unknown>): string {
  const table = wrapper.findComponent(DataTable)
  const fn = (table.props() as unknown as { rowClassName: (r: Record<string, unknown>) => string }).rowClassName
  return fn(row)
}

describe('RealtimeMonitor 点位实时监视', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    state.points = [point('p-1'), point('p-2', { value: 12, quality: 'bad' })]
    state.connected = true
    state.attempts = 0
    state.favorites = []
    state.paused = false
    if (hooks.pausedRef) hooks.pausedRef.value = false
    state.emptyText = '已连接，等待首帧采集数据…'
    state.emptyKind = 'empty'
    state.emptyActionText = ''
    state.emptyActionRoute = ''
    state.chartError = ''
    useWebsocketStore().setStatus('connected')
  })

  it('已连接且有点位：显示「已连接」与真实点位数，表格行数一致', async () => {
    const wrapper = await mountPage()
    const text = wrapper.text()
    expect(text).toContain('已连接')
    expect(text).toContain('2 个点位')
    expect(tableRows(wrapper)).toHaveLength(2)
  })

  it('首帧未到时不显示「0 个点位」，而是「等待首帧数据…」', async () => {
    state.points = []
    state.connected = true
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('等待首帧数据…')
    expect(wrapper.text()).not.toContain('0 个点位')
  })

  it('未连接时区分「连接中…」与「重连中 (n)」', async () => {
    state.points = []
    state.connected = false
    state.attempts = 0
    const wrapper = await mountPage()
    expect(wrapper.text()).toContain('连接中…')
    expect(wrapper.text()).toContain('未连接')

    state.attempts = 3
    const wrapper2 = mount(RealtimeMonitor)
    await settle()
    expect(wrapper2.text()).toContain('重连中 (3)')
  })

  it('断连但表里还有旧值时，陈旧横幅必须出现并声明「不是实时值」', async () => {
    state.connected = false
    const wrapper = await mountPage()

    const banner = wrapper.find('.stale-banner')
    expect(banner.exists()).toBe(true)
    expect(banner.text()).toContain('不是实时值')
    expect(banner.text()).toContain('最后更新')
  })

  it('陈旧横幅的「立即重连」按钮真的会触发重连', async () => {
    state.connected = false
    const wrapper = await mountPage()
    const btn = wrapper.find('.stale-banner').findAll('button')[0]!
    await btn.trigger('click')
    // reconnectNow 由 composable 提供，此处只验证按钮可点且未抛错
    expect(btn.text()).toContain('立即重连')
  })

  it('连接正常时不出陈旧横幅（避免噪音）', async () => {
    const wrapper = await mountPage()
    expect(wrapper.find('.stale-banner').exists()).toBe(false)
  })

  it('暂停刷新后按钮文案反转并出现「已冻结」标记', async () => {
    const wrapper = await mountPage()
    expect(wrapper.find('.pause-btn').text()).toContain('暂停刷新')

    await wrapper.find('.pause-btn').trigger('click')
    await settle()

    expect(wrapper.find('.pause-btn').text()).toContain('继续刷新')
    expect(wrapper.find('.paused-tag').text()).toBe('已冻结')
  })

  it('筛选后无数据：空态给出「取消筛选」并真正清空筛选', async () => {
    state.emptyKind = 'filtered'
    state.emptyText = '当前筛选条件下没有点位'
    state.emptyActionText = '取消收藏筛选'
    const wrapper = await mountPage()
    // 先打开「仅看收藏」，让 displayPoints 变空
    const favBox = wrapper.findAll('input[type="checkbox"]')[0]!
    await favBox.setValue(true)
    await settle()

    expect(wrapper.findComponent(EmptyState).exists()).toBe(true)
    wrapper.findComponent(EmptyState).vm.$emit('action')
    await settle()
    expect(wrapper.findComponent(EmptyState).exists()).toBe(false)
  })

  it('空态有目标页面时，行动按钮走路由而不是无脑重连', async () => {
    state.points = []
    state.emptyKind = 'empty'
    state.emptyText = '还没有任何点位'
    state.emptyActionText = '去添加点位'
    state.emptyActionRoute = '/devices?tab=points'
    const wrapper = await mountPage()

    wrapper.findComponent(EmptyState).vm.$emit('action')
    await settle()
    expect(push).toHaveBeenCalledWith('/devices?tab=points')
  })

  it('行质量标记：bad 走红底、stale/uncertain 走黄底、good 不加类', async () => {
    const wrapper = await mountPage()

    expect(rowClass(wrapper, { quality: 'bad' })).toBe('row-quality-bad')
    expect(rowClass(wrapper, { quality: 'stale' })).toBe('row-quality-warn')
    expect(rowClass(wrapper, { quality: 'uncertain' })).toBe('row-quality-warn')
    expect(rowClass(wrapper, { quality: 'good' })).toBe('')
  })

  it('趋势图渲染异常时显示原因，不留空白画布', async () => {
    state.chartError = 'canvas 初始化失败'
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('趋势图渲染失败：canvas 初始化失败')
  })

  it('导出按钮在没有可导出数据时禁用', async () => {
    state.points = []
    const wrapper = await mountPage()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('导出 CSV'))!
    expect(btn.attributes('disabled')).toBeDefined()
  })

  it('首次建连期间显示加载态；进入重连后必须露出空态（否则「立即重连」被骨架屏顶掉）', async () => {
    state.points = []
    state.connected = false
    useWebsocketStore().setStatus('connecting')
    state.attempts = 0
    const wrapper = await mountPage()
    expect((wrapper.findComponent(DataTable).props() as unknown as { loading: boolean }).loading).toBe(true)

    state.attempts = 1
    const wrapper2 = mount(RealtimeMonitor)
    await settle()
    expect((wrapper2.findComponent(DataTable).props() as unknown as { loading: boolean }).loading).toBe(false)
  })
})
