import { describe, it, expect, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { createMemoryHistory, createRouter } from 'vue-router'
import Sidebar from './Sidebar.vue'

/** Sidebar 测试：菜单搜索过滤、defaultOpeneds 路由联动、折叠状态切换 */

async function setup(initialPath = '/dashboard') {
  setActivePinia(createPinia())
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/', component: { template: '<div />' } },
      { path: '/dashboard', component: { template: '<div />' } },
      { path: '/devices', component: { template: '<div />' } },
      { path: '/history', component: { template: '<div />' } },
      { path: '/system', component: { template: '<div />' } },
    ],
  })
  await router.push(initialPath)
  await router.isReady()
  const wrapper = mount(Sidebar, {
    props: { collapsed: false },
    global: { plugins: [router] },
  })
  return { wrapper, router }
}

describe('Sidebar', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('未折叠时显示 logo 文字与菜单搜索框', async () => {
    const { wrapper } = await setup()
    expect(wrapper.find('.logo-text').exists()).toBe(true)
    expect(wrapper.find('.menu-search').exists()).toBe(true)
  })

  it('折叠时隐藏 logo 文字与菜单搜索框', async () => {
    const { wrapper } = await setup()
    await wrapper.setProps({ collapsed: true } as Record<string, unknown>)
    // v-show 控制 display
    expect(wrapper.find('.logo-text').attributes('style')).toContain('display: none')
    expect(wrapper.find('.menu-search').attributes('style')).toContain('display: none')
  })

  it('菜单搜索过滤：输入关键词后仅展示匹配项', async () => {
    const { wrapper } = await setup()
    // 默认展示分组菜单
    const beforeCount = wrapper.findAll('.el-menu-item').length
    expect(beforeCount).toBeGreaterThan(0)
    // 输入搜索词
    const input = wrapper.find('.menu-search input')
    await input.setValue('设备')
    // 搜索态下渲染扁平菜单项
    const afterCount = wrapper.findAll('.el-menu-item').length
    expect(afterCount).toBeGreaterThan(0)
    expect(afterCount).toBeLessThanOrEqual(beforeCount)
    // 搜索结果补「业务域 › 功能名」路径，用来区分同名功能
    expect(wrapper.find('.menu-item-path').text()).toContain('›')
  })

  it('defaultOpeneds 包含当前路由所属分组', async () => {
    const { wrapper, router } = await setup('/dashboard')
    const exposed = wrapper.vm.$.exposed as { defaultOpeneds: { value: string[] } } | null
    expect(exposed).toBeTruthy()
    expect(exposed!.defaultOpeneds.value).toHaveLength(1)
    await router.push('/devices')
    await router.isReady()
    expect(exposed!.defaultOpeneds.value).toHaveLength(1)
  })

  it('activeMenu 与当前路径同步', async () => {
    const { wrapper, router } = await setup('/dashboard')
    const exposed = wrapper.vm.$.exposed as { activeMenu: { value: string } } | null
    expect(exposed!.activeMenu.value).toBe('/dashboard')
    await router.push('/devices')
    await router.isReady()
    expect(exposed!.activeMenu.value).toBe('/devices')
  })

  it('带 ?tab= 深链时分组菜单仍按纯路径高亮', async () => {
    // HubShell 挂载后会把 ?tab= 写进 URL（如 /devices?tab=devices），
    // 该值恰好是搜索索引 nav-devices 的 target；若 activeMenu 按 fullPath 匹配，
    // 会与菜单项纯路径 index 不一致，导致整栏无高亮（EP 精确匹配失败）。
    const { wrapper, router } = await setup('/devices')
    const exposed = wrapper.vm.$.exposed as { activeMenu: { value: string } } | null
    await router.push('/devices?tab=devices')
    await router.isReady()
    // 分组态（无搜索关键词）：必须回退到 route.path，而不是索引 target
    expect(exposed!.activeMenu.value).toBe('/devices')
  })
})