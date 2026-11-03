import { ref } from 'vue'
import { defineStore } from 'pinia'
import { logWarn } from '@/utils/logger'
import { globalGet, globalSet } from '@/utils/gatewayStorage'

/**
 * 主题是「这台浏览器上的人」的偏好，不是网关配置：切网关不重置主题，
 * 因此走 gatewayStorage 的全局入口（不按网关命名空间隔离），
 * 但仍集中在本模块访问 localStorage，便于将来整体替换存储实现。
 */
const STORAGE_KEY = 'gw_theme'

function loadInitialDark(): boolean {
  const saved = globalGet(STORAGE_KEY)
  if (saved === 'dark') return true
  if (saved === 'light') return false
  // 无本地记忆（首次使用）时跟随系统偏好
  return window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? false
}

/** 暗色 class 同步到文档根节点：EP 官方 dark css-vars（html.dark）与自研 .dark 变量块都挂在该 class 下 */
function applyDarkClass(dark: boolean): void {
  document.documentElement.classList.toggle('dark', dark)
}

/**
 * 主题切换钩子（修复 #1 / B-1-ROI #1）。
 *
 * 任何暗色状态变更后，调用 `applyEchartsTheme()` 让当前所有图表即时刷新
 * ——`Store` 不直接 import `utils/echarts`（避免图表模块给主题 store 拖入
 * 全量 echarts core / 5 个 chart 模块，store 是几乎所有页面的共有依赖），
 * 这里走动态 import + 错误隔离：
 *
 *   - `import('@/utils/echarts')` 走 Vite 已拆包，运行时才取图表工具。
 *   - 失败时 console.warn：缺 echarts 不会让主题切换崩溃（其他依赖项
 *     如 element-plus dark vars 仍正常工作）。
 */
function applyEchartsThemeSafe(dark: boolean): void {
  if (typeof window === 'undefined') return
  void import('@/utils/echarts')
    .then((m) => {
      if (typeof m.applyEchartsTheme === 'function') m.applyEchartsTheme(dark)
    })
    .catch((e) => {
      logWarn('theme', 'failed to apply echarts theme (non-fatal):', e)
    })
}

export const useThemeStore = defineStore('theme', () => {
  const isDark = ref<boolean>(loadInitialDark())

  /** 直接设置主题（跨标签 storage 同步用） */
  function setDark(dark: boolean): void {
    const prev = isDark.value
    isDark.value = dark
    applyDarkClass(dark)
    try { globalSet(STORAGE_KEY, dark ? 'dark' : 'light') } catch { /* ignore */ }
    // 仅在状态真的变更时才通知图表——避免一帧重复 dispatch（initTheme 首调已是目标态）
    if (prev !== dark) {
      applyEchartsThemeSafe(dark)
    }
  }

  /** 切换主题并持久化（顶栏按钮入口） */
  function toggleTheme(): void {
    setDark(!isDark.value)
  }

  /** 应用启动时调用：挂载前恢复上次主题，避免暗色用户首屏闪白 */
  function initTheme(): void {
    applyDarkClass(isDark.value)
    // 首屏挂载时也要 dispatch 一次：可能存在 theme switch 之前已经渲染的图表
    // （例如：用户从浅色进页面后立刻在页面顶部点暗色按钮，store 早就有实例）。
    // 用 isDark 当前态即可，因为此时主题变更 = 首屏之前已经发生过的事件不会再来。
    // 这一步不依赖状态转换，所以单独调用而非走 setDark。
    applyEchartsThemeSafe(isDark.value)
  }

  return { isDark, toggleTheme, setDark, initTheme }
})
