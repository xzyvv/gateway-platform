import type { Router } from 'vue-router'

/**
 * 路由 chunk 预取（侧栏菜单 / 搜索结果 hover、focus 时触发）。
 *
 * 网关跑在低配 ARM + eMMC 上，按需 import 的 chunk 首次读取有明显延迟。
 * 用户从悬停到点击通常有 150ms 以上，足够提前把目标页的 JS/CSS 下载好。
 * 预取只是加速：失败时静默移除记录，下次悬停再试，绝不影响正常导航。
 */
const prefetched = new Set<string>()

export function prefetchRoute(router: Router, path: string): void {
  if (!path || prefetched.has(path)) return
  prefetched.add(path)

  const resolved = router.resolve(path)
  const record = resolved.matched[resolved.matched.length - 1]
  const component = record?.components?.default
  // 本项目所有路由都是 () => import(...)；函数组件不会被当成 loader 反复调用
  if (typeof component !== 'function') return

  try {
    const result = (component as () => unknown)()
    if (result instanceof Promise) {
      result.catch(() => { prefetched.delete(path) })
    }
  } catch {
    prefetched.delete(path)
  }
}
