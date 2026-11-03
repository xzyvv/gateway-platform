import { describe, it, expect, beforeEach, vi } from 'vitest'
import { useTableDensity, useColumnPrefs, __resetTablePrefsForTests } from './useTablePrefs'

/**
 * 表格偏好（密度全局 + 列显隐按表）的契约：
 * 1. 密度是全站共享的单例，任意处切换其他表立即跟随；
 * 2. 列显隐按表隔离，A 表的隐藏状态不能串到 B 表；
 * 3. 存储损坏 / 隐私模式写入失败一律静默回退，不能让页面打不开。
 */
describe('表格密度偏好', () => {
  beforeEach(() => {
    localStorage.clear()
    __resetTablePrefsForTests()
  })

  it('setDensity 后全站共享同一份状态', () => {
    const a = useTableDensity()
    const b = useTableDensity()
    a.setDensity('compact')
    expect(b.density.value).toBe('compact')
    // readonly：外部不能直接改，否则绕过持久化
    expect(a.density.value).toBe('compact')
  })

  it('toggleDensity 在标准/紧凑之间来回切换', () => {
    const { density, toggleDensity } = useTableDensity()
    expect(density.value).toBe('default')
    toggleDensity()
    expect(density.value).toBe('compact')
    toggleDensity()
    expect(density.value).toBe('default')
  })

  it('持久化后重新初始化能读回紧凑', () => {
    useTableDensity().setDensity('compact')
    __resetTablePrefsForTests() // 清模块缓存，模拟重新进入页面
    expect(useTableDensity().density.value).toBe('compact')
  })

  it('损坏的本地值回退为标准，绝不因为一次坏写入把全站表格挤扁', () => {
    localStorage.setItem('gateway.table-prefs.density', JSON.stringify({ broken: true }))
    expect(useTableDensity().density.value).toBe('default')
    localStorage.setItem('gateway.table-prefs.density', 'compact-xxx')
    expect(useTableDensity().density.value).toBe('default')
  })

  it('写入失败（隐私模式）不抛错，本次会话内仍生效', () => {
    const { density, setDensity } = useTableDensity()
    const spy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError')
    })
    expect(() => setDensity('compact')).not.toThrow()
    expect(density.value).toBe('compact')
    spy.mockRestore()
  })
})

describe('列显隐偏好', () => {
  beforeEach(() => {
    localStorage.clear()
    __resetTablePrefsForTests()
  })

  it('切换隐藏状态并持久化', () => {
    const { hidden, isHidden, toggle } = useColumnPrefs('points')
    expect(isHidden('bus')).toBe(false)
    toggle('bus')
    expect(isHidden('bus')).toBe(true)
    expect(hidden.value).toEqual(['bus'])

    __resetTablePrefsForTests()
    expect(useColumnPrefs('points').isHidden('bus')).toBe(true)
  })

  it('按表隔离：A 表隐藏的列不影响 B 表', () => {
    useColumnPrefs('points').toggle('bus')
    // B 表本就没有 bus 列，若共享状态会被误隐藏；这里必须不受影响
    expect(useColumnPrefs('devices').hidden.value).toEqual([])
    expect(useColumnPrefs('devices').isHidden('bus')).toBe(false)
  })

  it('同一表多次引用共享同一份状态', () => {
    const a = useColumnPrefs('points')
    const b = useColumnPrefs('points')
    a.toggle('protocol')
    expect(b.isHidden('protocol')).toBe(true)
  })

  it('reset 恢复全部显示', () => {
    const { hidden, reset } = useColumnPrefs('points')
    hidden.value = ['a', 'b']
    reset()
    expect(hidden.value).toEqual([])
  })

  it('本地内容损坏时回退为全显示，不让表格空列', () => {
    localStorage.setItem('gateway.table-prefs.cols.points', '{不是合法 JSON')
    expect(useColumnPrefs('points').hidden.value).toEqual([])

    // 合法 JSON 但结构不对（对象而非数组）同样回退
    localStorage.setItem('gateway.table-prefs.cols.points', '{"bus":true}')
    expect(useColumnPrefs('points').hidden.value).toEqual([])
  })

  it('数组内的非字符串项被过滤，避免渲染出 undefined 列名', () => {
    localStorage.setItem('gateway.table-prefs.cols.points', JSON.stringify(['bus', 123, null, 'protocol']))
    expect(useColumnPrefs('points').hidden.value).toEqual(['bus', 'protocol'])
  })
})