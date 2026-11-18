import { describe, expect, it, vi } from 'vitest'
import { flashHighlightRow } from './tableHighlight'

/**
 * 契约：定位高亮行的定时器在卸载后不得再写 DOM。
 *
 * 原实现（设备清单 / 点位清单各写一遍）在组件卸载后定时器仍在跑，
 * 回调持有已移除的 DOM 引用；用户快速切页时多个 timer 叠加，
 * 各自往不同 DOM 上加/删 class。
 */
describe('flashHighlightRow', () => {
  it('100ms 后找到高亮行并加上闪烁 class', () => {
    vi.useFakeTimers()
    const row = document.createElement('div')
    row.className = 'el-table__row is-highlighted'
    row.scrollIntoView = vi.fn()
    document.body.innerHTML = ''
    document.body.appendChild(row)
    const container = document.createElement('div')
    container.className = 'test-table'
    container.appendChild(row)
    document.body.appendChild(container)

    const scroll = flashHighlightRow('.test-table')
    scroll()
    vi.advanceTimersByTime(100)

    expect(row.scrollIntoView).toHaveBeenCalled()
    expect(row.classList.contains('is-highlighted-flash')).toBe(true)
    vi.useRealTimers()
  })

  it('3 秒后移除闪烁 class', () => {
    vi.useFakeTimers()
    const row = document.createElement('div')
    row.className = 'el-table__row is-highlighted'
    row.scrollIntoView = vi.fn()
    const container = document.createElement('div')
    container.className = 'test-table'
    container.appendChild(row)
    document.body.innerHTML = ''
    document.body.appendChild(container)

    const scroll = flashHighlightRow('.test-table')
    scroll()
    vi.advanceTimersByTime(100)
    expect(row.classList.contains('is-highlighted-flash')).toBe(true)

    vi.advanceTimersByTime(3000)
    expect(row.classList.contains('is-highlighted-flash')).toBe(false)
    vi.useRealTimers()
  })

  it('卸载后不再查找 DOM，也不再写 class', () => {
    vi.useFakeTimers()
    let unmounted = false
    const row = document.createElement('div')
    row.className = 'el-table__row is-highlighted'
    row.scrollIntoView = vi.fn()
    const container = document.createElement('div')
    container.className = 'test-table'
    container.appendChild(row)
    document.body.innerHTML = ''
    document.body.appendChild(container)

    const scroll = flashHighlightRow('.test-table', () => unmounted)
    scroll()
    unmounted = true
    vi.advanceTimersByTime(100)

    // 卸载后不该再滚动、更不该加闪烁 class
    expect(row.scrollIntoView).not.toHaveBeenCalled()
    expect(row.classList.contains('is-highlighted-flash')).toBe(false)
    vi.useRealTimers()
  })

  it('重复调用时旧定时器被取消，不叠加多个闪烁', () => {
    vi.useFakeTimers()
    const row = document.createElement('div')
    row.className = 'el-table__row is-highlighted'
    row.scrollIntoView = vi.fn()
    const container = document.createElement('div')
    container.className = 'test-table'
    container.appendChild(row)
    document.body.innerHTML = ''
    document.body.appendChild(container)

    const scroll = flashHighlightRow('.test-table')
    scroll()
    scroll()
    scroll()
    vi.advanceTimersByTime(100)

    // 只滚动一次（后两次调用取消了前面的定时器）
    expect(row.scrollIntoView).toHaveBeenCalledTimes(1)
    vi.useRealTimers()
  })
})
