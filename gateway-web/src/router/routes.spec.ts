import { describe, it, expect } from 'vitest'
import type { RouteRecordRaw } from 'vue-router'
import { routes } from '@/router/routes'

/**
 * 路由元数据契约测试
 *
 * 路由表是「新增页面」时最容易漏改的地方（标题、管理员标记、懒加载、重定向目标），
 * 且漏了不会报错、只会静默表现成"标签页标题是空的"或"菜单点了没反应"。
 * 这里把四条约定固定下来，并用快照兜住 meta 的意外改动。
 */

/** 有组件定义的页面路由（排除 redirect-only 记录） */
const pageRoutes = routes.filter((r) => typeof r.component === 'function' || Array.isArray(r.component))

/** redirect-only 记录（路径别名跳转，如快捷入口） */
const redirectRoutes = routes.filter((r) => typeof r.redirect === 'string')

describe('路由元数据', () => {
  it('每个页面路由都有 name 和 meta.title', () => {
    const missing = pageRoutes
      .filter((r) => !r.name || !r.meta?.title)
      .map((r) => r.path)

    expect(missing).toEqual([])
  })

  it('path 与 name 全局唯一', () => {
    const paths = routes.map((r) => r.path)
    expect(new Set(paths).size).toBe(paths.length)

    const names = pageRoutes.map((r) => String(r.name))
    expect(new Set(names).size).toBe(names.length)
  })

  it('页面组件必须是懒加载函数（不得静态 import，破坏分包）', () => {
    // 静态组件会让该页面连同依赖进入首屏 chunk；与 vite.config 的分包策略冲突。
    const eager = pageRoutes.filter((r) => typeof r.component !== 'function').map((r) => r.path)
    expect(eager).toEqual([])
  })

  it('redirect 目标必须是已登记的路由路径', () => {
    const known = new Set(routes.map((r) => r.path))
    const broken = redirectRoutes
      .map((r) => ({ from: r.path, to: String(r.redirect).split('?')[0]! }))
      .filter(({ to }) => !known.has(to))

    expect(broken).toEqual([])
  })

  it('requireAdmin 若存在必须为 true（避免写成字符串导致守卫误判）', () => {
    const invalid = routes
      .filter((r) => 'requireAdmin' in (r.meta ?? {}))
      .filter((r) => (r.meta as { requireAdmin?: unknown }).requireAdmin !== true)
      .map((r) => r.path)

    expect(invalid).toEqual([])
  })

  it('meta 快照：路径 → 标题 / 管理员标记 / 重定向', () => {
    const snapshot = routes
      .map((r: RouteRecordRaw) => ({
        path: r.path,
        title: r.meta?.title ?? null,
        requireAdmin: (r.meta as { requireAdmin?: boolean } | undefined)?.requireAdmin ?? false,
        redirect: typeof r.redirect === 'string' ? r.redirect : null,
        lazy: typeof r.component === 'function',
      }))
      .sort((a, b) => a.path.localeCompare(b.path))

    expect(snapshot).toMatchSnapshot()
  })
})
