import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { applyReducedMotion } from './echarts'

/**
 * 回归测试：ECharts 尊重「减少动态效果」偏好（P2 #4a）。
 *
 * CSS 侧的 prefers-reduced-motion 兜底只管 DOM 动画/过渡；ECharts 的入场动画、
 * 逐点动画是 canvas 内 JS 驱动渲染，CSS 完全管不到——前庭障碍用户在系统里
 * 开启「减少动态效果」后，图表仍会播放折线生长/柱状增长动画。
 *
 * 这里只验证纯函数层（option 补丁），不碰真实 echarts 渲染。
 */

/** 覆盖 window.matchMedia，使指定查询返回给定 matches */
function stubMatchMedia(matches: boolean) {
  const spy = vi.fn((query: string) => ({
    matches: query.includes('prefers-reduced-motion') ? matches : false,
    media: query,
    onchange: null,
    addListener: vi.fn(),
    removeListener: vi.fn(),
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
    dispatchEvent: vi.fn(),
  }))
  Object.defineProperty(window, 'matchMedia', { value: spy, writable: true, configurable: true })
  return spy
}

describe('applyReducedMotion', () => {
  let originalMatchMedia: typeof window.matchMedia

  beforeEach(() => {
    originalMatchMedia = window.matchMedia
  })

  afterEach(() => {
    Object.defineProperty(window, 'matchMedia', {
      value: originalMatchMedia,
      writable: true,
      configurable: true,
    })
    vi.restoreAllMocks()
  })

  it('命中「减少动态效果」时关闭全部动画', () => {
    stubMatchMedia(true)
    const option = applyReducedMotion({ series: [{ type: 'line' }] })
    expect(option.animation).toBe(false)
    expect(option.animationDuration).toBe(0)
    expect(option.animationDurationUpdate).toBe(0)
    expect(option.animationDelay).toBe(0)
    expect(option.animationDelayUpdate).toBe(0)
  })

  it('未命中时原样返回（不改动用户配置）', () => {
    stubMatchMedia(false)
    const original = { series: [{ type: 'line' }], animationDuration: 1000 }
    const option = applyReducedMotion(original)
    expect(option).toBe(original)
    expect(option.animationDuration).toBe(1000)
  })

  it('命中时保留原有业务配置，只覆盖动画相关字段', () => {
    stubMatchMedia(true)
    const option = applyReducedMotion({
      series: [{ type: 'line', data: [1, 2, 3] }],
      tooltip: { trigger: 'axis' },
    })
    expect(option.series).toEqual([{ type: 'line', data: [1, 2, 3] }])
    expect(option.tooltip).toEqual({ trigger: 'axis' })
  })

  it('不污染调用方传入的对象（返回浅拷贝）', () => {
    stubMatchMedia(true)
    const original = { series: [], animationDuration: 1000 }
    const option = applyReducedMotion(original)
    expect(option).not.toBe(original)
    expect(original.animationDuration).toBe(1000)
  })

  it('matchMedia 抛错时安全降级为不启用（不阻断图表渲染）', () => {
    Object.defineProperty(window, 'matchMedia', {
      value: () => { throw new Error('unsupported') },
      writable: true,
      configurable: true,
    })
    const original = { animationDuration: 500 }
    expect(applyReducedMotion(original)).toBe(original)
  })

  it('matchMedia 不存在时安全降级（SSR / 老环境）', () => {
    Object.defineProperty(window, 'matchMedia', {
      value: undefined,
      writable: true,
      configurable: true,
    })
    const original = { animationDuration: 500 }
    expect(applyReducedMotion(original)).toBe(original)
  })
})
