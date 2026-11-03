import { describe, it, expect, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import TablePrefsMenu from './TablePrefsMenu.vue'
import { useTableDensity, useColumnPrefs, __resetTablePrefsForTests } from '@/composables/core/useTablePrefs'

const columns = [
  { key: 'name', title: '名称' },
  { key: 'bus', title: '总线' },
  { key: 'address', title: '地址' },
]

/**
 * 偏好面板的行为契约：
 * 1. 面板内的密度切换会真正改变全站共享偏好（不是页面内的临时状态）；
 * 2. 列显隐按 tableId 记忆，两个表用同一组件不互相污染；
 * 3. 最后一列不允许再隐藏——全空的表格会被用户当成"页面坏了"。
 */
describe('TablePrefsMenu', () => {
  beforeEach(() => {
    localStorage.clear()
    __resetTablePrefsForTests()
    document.body.innerHTML = ''
  })

  function mountMenu(tableId = 'points', cols = columns) {
    return mount(TablePrefsMenu, { props: { tableId, columns: cols }, attachTo: document.body })
  }

  it('渲染带无障碍标签的设置入口', () => {
    const wrapper = mountMenu()
    expect(wrapper.find('button[aria-label="表格显示设置"]').exists()).toBe(true)
    wrapper.unmount()
  })

  it('点开后同时提供密度与列显隐两项', async () => {
    const wrapper = mountMenu()
    await wrapper.find('button[aria-label="表格显示设置"]').trigger('click')
    const body = document.body.textContent || ''
    expect(body).toContain('行密度')
    expect(body).toContain('标准')
    expect(body).toContain('紧凑')
    expect(body).toContain('显示列')
    expect(body).toContain('名称')
    wrapper.unmount()
  })

  it('不传 columns 时不显示空的「显示列」区', async () => {
    const wrapper = mount(TablePrefsMenu, { props: { tableId: 'plain' }, attachTo: document.body })
    await wrapper.find('button[aria-label="表格显示设置"]').trigger('click')
    expect(document.body.textContent || '').not.toContain('显示列')
    wrapper.unmount()
  })

  it('面板里的密度切换写入全站共享偏好', async () => {
    const wrapper = mountMenu()
    await wrapper.find('button[aria-label="表格显示设置"]').trigger('click')

    const compact = Array.from(document.querySelectorAll('.el-radio-button'))
      .find((el) => el.textContent?.includes('紧凑'))
    expect(compact).toBeTruthy()
    compact!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await wrapper.vm.$nextTick()

    expect(useTableDensity().density.value).toBe('compact')
    wrapper.unmount()
  })

  it('列显隐按 tableId 记忆，两个表互不污染', () => {
    const a = mountMenu('points')
    a.unmount()
    const b = mountMenu('devices')
    b.unmount()
    // 两个表共享同一个 store 入口，但缓存按 tableId 分开
    useColumnPrefs('points').toggle('bus')
    expect(useColumnPrefs('points').isHidden('bus')).toBe(true)
    expect(useColumnPrefs('devices').isHidden('bus')).toBe(false)
  })

  it('隐藏到只剩一列时，最后一列仍可点回显示（不允许把表格清空）', async () => {
    const wrapper = mountMenu()
    await wrapper.find('button[aria-label="表格显示设置"]').trigger('click')

    const p = useColumnPrefs('points')
    p.toggle('name')
    p.toggle('bus')
    await wrapper.vm.$nextTick()

    // 只剩 address 一列可见；面板必须仍把它当作可恢复项展示出来，
    // 否则用户隐藏过头后就没有回头路了
    expect(p.isHidden('address')).toBe(false)
    expect(p.hidden.value).toEqual(['name', 'bus'])
    const body = document.body.textContent || ''
    expect(body).toContain('地址')
    wrapper.unmount()
  })

  it('「全部显示」只复位列，不动密度', async () => {
    const wrapper = mountMenu()
    await wrapper.find('button[aria-label="表格显示设置"]').trigger('click')

    useColumnPrefs('points').toggle('name')
    useTableDensity().setDensity('compact')
    await wrapper.vm.$nextTick()

    const reset = Array.from(document.querySelectorAll('button'))
      .find((el) => el.textContent?.includes('全部显示'))
    expect(reset).toBeTruthy()
    reset!.dispatchEvent(new MouseEvent('click', { bubbles: true }))
    await wrapper.vm.$nextTick()

    expect(useColumnPrefs('points').hidden.value).toEqual([])
    expect(useTableDensity().density.value).toBe('compact')
    wrapper.unmount()
  })
})