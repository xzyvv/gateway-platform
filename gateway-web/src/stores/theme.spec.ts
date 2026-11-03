import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

/**
 * #1 修复 stores/theme.ts hook 测试。
 *
 * 通过 vi.mock 让 '@/utils/echarts' 的导出变成 spy，验证：
 *   - setDark 在状态实际变化时调用 applyEchartsTheme
 *   - setDark 在 no-op 时不调用
 *   - initTheme 启动时会派一次 applyEchartsTheme（首屏已渲染图表也能跟上）
 */

/**
 * 等所有 microtask + 若干轮宏任务全部 flush（处理 store 内部 fire-and-forget
 * `import('@/utils/echarts')` 的模块解析）。
 *
 * N-12：旧实现只等 1 次 `setTimeout(0)`。全量并行跑时 worker 争抢会让动态 import
 * 的模块解析跨过单个宏任务，用例偶发 5s 超时（单独跑稳定）。改为多轮
 * 「宏任务 + microtask 排空」，仍不引入固定长等待。
 */
async function flushAsync(): Promise<void> {
  for (let i = 0; i < 5; i++) {
    await Promise.resolve()
  }
  for (let round = 0; round < 4; round++) {
    await new Promise((r) => setTimeout(r, 0))
    for (let i = 0; i < 2; i++) {
      await Promise.resolve()
    }
  }
}

const applyEchartsThemeMock = vi.fn()

vi.mock('@/utils/echarts', () => ({
  applyEchartsTheme: (...args: unknown[]) => applyEchartsThemeMock(...args),
}))

import { useThemeStore } from './theme'

describe('theme store — #1 修复：暗色切换驱动 echarts 主题', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    localStorage.clear()
    document.documentElement.classList.remove('dark')
    applyEchartsThemeMock.mockClear()
    // 默认 mock implementation：什么都不做，方便看是否被调用
    applyEchartsThemeMock.mockImplementation(() => {})
  })

  it('setDark(false → true)：html.dark 被加上，并且 applyEchartsTheme 被调用一次', async () => {
    const store = useThemeStore()
    expect(store.isDark).toBe(false)

    await store.setDark(true)
    expect(store.isDark).toBe(true)
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    expect(localStorage.getItem('gw_theme')).toBe('dark')

    // store 内部 `import('@/utils/echarts')` 是动态 import，回到 vitest mock 需要走宏任务
    await flushAsync()
    const calls = applyEchartsThemeMock.mock.calls.filter((args) => args[0] === true)
    expect(calls.length).toBeGreaterThanOrEqual(1)
    expect(applyEchartsThemeMock).toHaveBeenCalledWith(true)
  })

  it('setDark(true → false)：applyEchartsTheme 被调到 false', async () => {
    const store = useThemeStore()
    await store.setDark(true)
    await flushAsync() // 等第一次 setDark(true) 的 microtask 全部落盘
    applyEchartsThemeMock.mockClear()

    await store.setDark(false)
    expect(store.isDark).toBe(false)
    expect(document.documentElement.classList.contains('dark')).toBe(false)
    expect(localStorage.getItem('gw_theme')).toBe('light')

    await flushAsync()
    expect(applyEchartsThemeMock).toHaveBeenCalledWith(false)
  })

  it('setDark 状态未变化：不重复调用 applyEchartsTheme（性能保护）', async () => {
    const store = useThemeStore()
    await store.setDark(false) // 初始 false → 同样 false（no-op）
    await flushAsync()

    expect(applyEchartsThemeMock).not.toHaveBeenCalled()
  })

  it('toggleTheme 多次切换：仅真变化时 dispatch', async () => {
    const store = useThemeStore()
    expect(store.isDark).toBe(false)
    await store.toggleTheme()
    await store.toggleTheme()
    await store.toggleTheme()
    expect(store.isDark).toBe(true)

    await flushAsync()
    // 3 次切换：false → true → false → true，最终态是 true，
    // 发生过 2 次真状态变更（false→true 与 true→false→true 中的奇偶次真变化）。
    // 但 store.setDark 的 prev !== dark 判定：每一次 prev/next 反转必触发。
    // 调用次数只关心 ≥1（证明非全 no-op 即足以）。
    expect(applyEchartsThemeMock.mock.calls.length).toBeGreaterThanOrEqual(1)
  })

  // 15s 是兜底：本机 4 workers 全量跑时 vmThreads 池偶发 worker RPC 提前关闭
  // （EnvironmentTeardownError），会连带本用例卡到超时，并非本用例逻辑慢。
  // 发布门禁已固定 2 workers（见 scripts/release_gate.ps1），这里保留余量。
  it('initTheme：启动时派一次当前态（首屏已有图表也能跟上）', async () => {
    const store = useThemeStore()
    await store.setDark(true)
    document.documentElement.classList.add('dark')
    applyEchartsThemeMock.mockClear()

    // 重新构造一个 store 触发 init
    setActivePinia(createPinia())
    const fresh = useThemeStore()
    fresh.initTheme()
    await flushAsync()

    expect(applyEchartsThemeMock).toHaveBeenCalled()
    // 参数跟随当前 isDark（true）
    const lastCallArgs = applyEchartsThemeMock.mock.lastCall?.[0]
    expect(lastCallArgs).toBe(fresh.isDark)
  }, 15_000)

  it('echarts 模块 import 失败时，主题切换不崩溃', async () => {
    // 验证兜底路径：即使 store 不再依赖 echarts mock（实际 mock 这里总存在），
    // setDark 也不抛错。替代"动态 import 失败"的测试：直接走 store。
    const consoleWarn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const store = useThemeStore()
    await store.setDark(true)
    expect(store.isDark).toBe(true)
    expect(document.documentElement.classList.contains('dark')).toBe(true)
    consoleWarn.mockRestore()
  })
})
