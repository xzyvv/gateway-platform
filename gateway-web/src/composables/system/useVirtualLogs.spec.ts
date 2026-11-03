import { describe, it, expect } from 'vitest'
import {
  buildPrefix,
  findStartIndex,
  scrollTopRange,
  tailRange,
  ESTIMATED_ROW_HEIGHT,
} from './useVirtualLogs'

/** 构造 n 条日志，id 从 1 递增（与 useLogs 的 ++logIdCounter 语义一致） */
function makeItems(n: number) {
  return Array.from({ length: n }, (_, i) => ({ id: i + 1, text: `line ${i + 1}` }))
}

describe('buildPrefix', () => {
  it('前缀和：实测高度优先，未实测行用估算值兜底', () => {
    const items = makeItems(3)
    const heights = new Map([[2, 40]]) // 第二行换行实测 40px
    const prefix = buildPrefix(items, heights)
    // [0, est, est+40, est+40+est]
    expect(prefix[1]).toBe(ESTIMATED_ROW_HEIGHT)
    expect(prefix[2]).toBe(ESTIMATED_ROW_HEIGHT + 40)
    expect(prefix[3]).toBe(ESTIMATED_ROW_HEIGHT * 2 + 40)
  })

  it('空列表：prefix 只有 0', () => {
    expect(buildPrefix([], new Map())).toEqual([0])
  })
})

describe('findStartIndex', () => {
  it('定位 scrollTop 所在行（二分，返回值可为 n 需调用方 clamp）', () => {
    // 三行各 22px：prefix = [0, 22, 44, 66]
    const prefix = buildPrefix(makeItems(3), new Map())
    expect(findStartIndex(prefix, 0)).toBe(0)
    expect(findStartIndex(prefix, 21)).toBe(0)
    expect(findStartIndex(prefix, 22)).toBe(1)
    expect(findStartIndex(prefix, 65)).toBe(2)
    // 滚出底部：返回 n，scrollTopRange 内部 clamp
    expect(findStartIndex(prefix, 1000)).toBe(3)
  })
})

describe('scrollTopRange', () => {
  const items = makeItems(100)
  const prefix = buildPrefix(items, new Map())

  it('顶部：窗口从头开始，含向下 overscan', () => {
    const r = scrollTopRange(prefix, 0, 220)
    expect(r.start).toBe(0)
    // 覆盖 220px 视口（10 行）+ overscan 10 行
    expect(r.end).toBeGreaterThanOrEqual(10)
  })

  it('中部：窗口覆盖视口且上下有 overscan 余量', () => {
    const scrollTop = 50 * ESTIMATED_ROW_HEIGHT // 第 50 行顶部
    const r = scrollTopRange(prefix, scrollTop, 220)
    expect(r.start).toBeGreaterThanOrEqual(40) // 50 - overscan 10
    expect(r.start).toBeLessThanOrEqual(50)
    expect(r.end).toBeGreaterThan(50)
    expect(r.end).toBeLessThanOrEqual(100)
  })

  it('滚出底部：clamp 到末行，不越界', () => {
    const r = scrollTopRange(prefix, 1e9, 220)
    expect(r.start).toBeLessThan(100)
    expect(r.end).toBe(100)
  })

  it('空列表：零窗口', () => {
    expect(scrollTopRange([0], 0, 220)).toEqual({ start: 0, end: 0 })
  })
})

describe('tailRange', () => {
  it('从尾部反推窗口：end 恒为 n，向上凑满视口 + overscan', () => {
    const items = makeItems(100)
    const prefix = buildPrefix(items, new Map())
    const r = tailRange(prefix, 220)
    expect(r.end).toBe(100)
    // 220px 视口 + 10×22 overscan = 440px，恰好 20 行（含尾行）→ start = 81
    expect(r.start).toBe(81)
  })

  it('行数少于视口：全量窗口', () => {
    const prefix = buildPrefix(makeItems(5), new Map())
    expect(tailRange(prefix, 220)).toEqual({ start: 0, end: 5 })
  })
})
