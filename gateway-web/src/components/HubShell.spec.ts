/**
 * HubShell 分组 + 受控卸载的行为契约。
 *
 * 背景：element-plus 的 `el-tabs lazy` 只保证「首次激活才渲染」，`loaded` 一旦置真
 * 永不复位（仅 v-show 隐藏）。于是用户点过的每个 tab 都长期挂载，轮询定时器 /
 * echarts 实例 / WebSocket 监听全部跟着常驻。本次改造改为分组左侧导航 + v-if
 * 只渲染当前 tab，切走即卸载。
 *
 * 这里锁住四件事：
 *   1. 渲染左侧分组导航，切换后旧 tab 组件真的被卸载（onUnmounted 被调用）
 *   2. 有未保存更改且用户拒绝放弃时，不切换、不卸载
 *   3. 页签很少（≤ 5）时同样使用左侧导航，不再切换成顶部标签条（全站单一范式）
 *   4. URL ?tab= 驱动的切换同样过脏检查，拒绝时把地址栏拉回当前 tab
 * 变异验证：删掉 `.hub-content` 上的 v-if（改为常驻）→ 用例 1 失败；
 * 删掉 selectTab 里的 handleBeforeLeave → 用例 2 失败；
 * 删掉 route watcher 里的 confirmDiscardDirty → 用例 4 失败。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { defineComponent, h, markRaw, onMounted, onUnmounted } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

const m = vi.hoisted(() => ({
  route: { path: '/system', query: {} as Record<string, string>, meta: { title: '系统设置' } },
  replace: vi.fn(),
  confirm: vi.fn(async (_scope: string) => true),
}))

vi.mock('vue-router', async () => {
  const { reactive } = await import('vue')
  m.route = reactive({
    path: '/system',
    query: {} as Record<string, string>,
    meta: { title: '系统设置' },
  })
  return {
    useRoute: () => m.route,
    useRouter: () => ({ replace: m.replace }),
  }
})
vi.mock('@/utils/dirtyGuard', () => ({
  confirmDiscardDirty: (...a: unknown[]) => m.confirm(...(a as [string])),
}))

import HubShell from './HubShell.vue'
import type { HubTab } from './HubShell.vue'
import { useAuthStore } from '@/stores/auth'

const mounts: Record<string, number> = {}
const unmounts: Record<string, number> = {}

function probe(name: string) {
  return defineComponent({
    name: `Probe-${name}`,
    setup() {
      onMounted(() => { mounts[name] = (mounts[name] ?? 0) + 1 })
      onUnmounted(() => { unmounts[name] = (unmounts[name] ?? 0) + 1 })
      return () => h('div', { class: `probe-${name}` }, name)
    },
  })
}

// 6 个页签 + 两个分组：验证分组折叠与受控卸载
function makeTabs(): HubTab[] {
  return [
    { name: 'a', label: '甲', group: 'g1', component: markRaw(probe('a')) },
    { name: 'b', label: '乙', group: 'g2', component: markRaw(probe('b')) },
    { name: 'c', label: '丙', group: 'g2', component: markRaw(probe('c')) },
    { name: 'd', label: '丁', group: 'g2', component: markRaw(probe('d')) },
    { name: 'e', label: '戊', group: 'g2', component: markRaw(probe('e')) },
    { name: 'f', label: '己', group: 'g2', component: markRaw(probe('f')) },
  ]
}

function mountShell(tabs: HubTab[] = makeTabs()) {
  return mount(HubShell, {
    props: {
      tabs,
      fallbackTab: tabs[0]?.name ?? 'a',
      groupLabels: { g1: '第一组', g2: '第二组' },
    },
  })
}

function navItem(wrapper: ReturnType<typeof mountShell>, label: string) {
  const item = wrapper.findAll('.hub-nav-item').find((b) => b.text() === label)
  if (!item) throw new Error(`未找到分组导航项：${label}`)
  return item
}

describe('HubShell 分组 + 受控卸载', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    m.route.query = {}
    m.confirm.mockImplementation(async () => true)
    localStorage.clear()
    for (const k of Object.keys(mounts)) delete mounts[k]
    for (const k of Object.keys(unmounts)) delete unmounts[k]
    useAuthStore().roleId = 'admin'
  })

  it('分组可折叠：收起后隐藏该组页签，aria-expanded 同步', async () => {
    const wrapper = mountShell()
    await flushPromises()

    const header = wrapper.findAll('.hub-nav-group-label')[0]!
    expect(header.attributes('aria-expanded')).toBe('true')

    await header.trigger('click')
    expect(header.attributes('aria-expanded')).toBe('false')
    // g1 只有「甲」，收起后导航只剩 g2 的五项
    expect(wrapper.findAll('.hub-nav-item').map((n) => n.text())).toEqual(['乙', '丙', '丁', '戊', '己'])
    // 当前 tab 的内容不受影响（折叠只是导航视觉收纳）
    expect(wrapper.find('.probe-a').exists()).toBe(true)
    // 状态落盘：卸载后重进仍保持折叠
    expect(JSON.parse(localStorage.getItem('gw-hub-collapsed:/system')!)).toEqual({ g1: true })

    await header.trigger('click')
    expect(header.attributes('aria-expanded')).toBe('true')
    expect(wrapper.findAll('.hub-nav-item')).toHaveLength(6)
    wrapper.unmount()
  })

  it('页签 ≥ 6 渲染左侧导航，切换后旧 tab 组件被卸载', async () => {
    const wrapper = mountShell()
    await flushPromises()

    expect(wrapper.find('.hub-nav').exists()).toBe(true)
    // 分组标签渲染
    expect(wrapper.findAll('.hub-nav-group-label').map((n) => n.text())).toEqual(['第一组', '第二组'])
    expect(wrapper.findAll('.hub-nav-item')).toHaveLength(6)
    // 只有当前 tab 挂载
    expect(wrapper.find('.probe-a').exists()).toBe(true)
    expect(wrapper.find('.probe-b').exists()).toBe(false)
    expect(mounts).toEqual({ a: 1 })

    // 页头 H1 与菜单名 / meta.title 对齐（= Hub 名），当前页签名交给左侧导航的选中态
    expect(wrapper.find('.hub-body-parent').exists()).toBe(false)
    expect(wrapper.find('.hub-body-title').text()).toBe('系统设置')
    expect(wrapper.find('h1.hub-body-title').exists()).toBe(true)

    await navItem(wrapper, '乙').trigger('click')
    await flushPromises()

    expect(wrapper.find('.probe-b').exists()).toBe(true)
    expect(wrapper.find('.probe-a').exists()).toBe(false)
    expect(unmounts.a).toBe(1) // 切走即卸载，定时器/图表/连接才能释放
    wrapper.unmount()
  })

  it('有未保存更改且用户拒绝放弃时，不切换也不卸载', async () => {
    m.confirm.mockImplementation(async () => false)
    const wrapper = mountShell()
    await flushPromises()

    await navItem(wrapper, '乙').trigger('click')
    await flushPromises()

    expect(m.confirm).toHaveBeenCalledWith('/system')
    expect(wrapper.find('.probe-a').exists()).toBe(true)
    expect(wrapper.find('.probe-b').exists()).toBe(false)
    expect(unmounts.a).toBeUndefined()
    wrapper.unmount()
  })

  it('页签 ≤ 5 仍使用左侧导航（全站单一范式），切换后旧 tab 同样被卸载', async () => {
    const tabs = makeTabs().slice(0, 3).map((t) => ({ ...t, group: undefined }))
    const wrapper = mountShell(tabs)
    await flushPromises()

    // P2-6：页签少时不再退化成顶部横向标签条
    expect(wrapper.find('.hub-nav').exists()).toBe(true)
    expect(wrapper.find('.el-tabs').exists()).toBe(false)
    expect(wrapper.findAll('.hub-nav-item')).toHaveLength(3)
    // 未声明 group 的页签不渲染分组标题（不把占位名 __default__ 暴露给用户）
    expect(wrapper.findAll('.hub-nav-group-label')).toHaveLength(0)
    // 页头 H1 取路由 meta.title
    expect(wrapper.find('.hub-body-title').text()).toBe('系统设置')

    await wrapper.findAll('.hub-nav-item')[1]!.trigger('click')
    await flushPromises()
    expect(wrapper.find('.probe-b').exists()).toBe(true)
    expect(wrapper.find('.probe-a').exists()).toBe(false)
    expect(unmounts.a).toBe(1)
    wrapper.unmount()
  })

  it('URL ?tab= 切换同样过脏检查，拒绝时地址栏拉回当前 tab', async () => {
    const wrapper = mountShell()
    await flushPromises()

    // 同意：切换并卸载旧 tab
    m.route.query.tab = 'b'
    await flushPromises()
    expect(wrapper.find('.probe-b').exists()).toBe(true)
    expect(unmounts.a).toBe(1)

    // 拒绝：保持当前 tab，并把 URL 改回去
    m.confirm.mockImplementation(async () => false)
    m.route.query.tab = 'c'
    await flushPromises()
    expect(wrapper.find('.probe-b').exists()).toBe(true)
    expect(wrapper.find('.probe-c').exists()).toBe(false)
    expect(m.replace).toHaveBeenCalledWith({ query: { tab: 'b' } })
    wrapper.unmount()
  })

  it('URL 指向不可见 tab（非管理员）时回退首个可见 tab', async () => {
    useAuthStore().roleId = 'operator'
    const tabs = makeTabs().map((t) => (t.name === 'c' ? { ...t, requireAdmin: true } : t))
    m.route.query.tab = 'c'
    const wrapper = mountShell(tabs)
    await flushPromises()

    // c 被过滤，回退到首个可见 tab a（不能渲染一个不存在的 tab）
    expect(wrapper.find('.probe-a').exists()).toBe(true)
    expect(wrapper.find('.probe-c').exists()).toBe(false)
    // 可见页签只剩 5 个，仍使用左侧导航（不再随数量切换范式）
    expect(wrapper.find('.hub-nav').exists()).toBe(true)
    expect(wrapper.findAll('.hub-nav-item')).toHaveLength(5)
    // 地址栏必须跟着回退，否则刷新/分享会再次落到不可见的 tab 上
    expect(m.replace).toHaveBeenCalledWith({ query: { tab: 'a' } })
    wrapper.unmount()
  })

  it('URL 带无效 tab（拼错）时把地址栏改写为实际显示的 tab', async () => {
    m.route.query.tab = 'nope'
    const wrapper = mountShell()
    await flushPromises()

    // 界面回退到首个页签
    expect(wrapper.find('.probe-a').exists()).toBe(true)
    // URL 与界面保持一致：改写为 a，而不是把无效值留在地址栏里
    expect(m.replace).toHaveBeenCalledWith({ query: { tab: 'a' } })
    wrapper.unmount()
  })
})
