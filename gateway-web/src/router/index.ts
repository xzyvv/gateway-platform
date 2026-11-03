import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router'
import { useAuthStore } from '../stores/auth'
import { routes as pageRoutes } from './routes'
import { isScopeDirty, confirmDiscardDirty } from '../utils/dirtyGuard'
import { logError } from '../utils/logger'

/**
 * 完整路由拼装（v2.0：全部注册，不再按 tier 过滤）。
 *
 * v1.2 曾在注册时按 tier 过滤 + beforeEach 守卫双重拦截高端页面；
 * v2.0 调试/分析/批量功能对所有 tier（含失效态）免费，路由不再携带 tier，
 * 页面路由统一在 routes/index.ts 维护（title/requireAdmin 直接写在 meta 中）。
 * 新增页面登记清单见 routes/index.ts 头部注释。
 */

const routes: RouteRecordRaw[] = [
  {
    path: '/login',
    name: 'Login',
    component: () => import('../views/auth/Login.vue'),
    meta: { public: true },
  },
  {
    path: '/change-password',
    name: 'ChangePassword',
    component: () => import('../views/auth/ChangePassword.vue'),
    meta: { title: '修改密码' },
  },
  // 路由层动态 import 失败兜底页（顶层路由，全屏渲染；不进 Layout）
  {
    path: '/load-failed',
    name: 'LoadFailed',
    component: () => import('../views/LoadFailedRoute.vue'),
    meta: { public: true, title: '加载失败' },
  },
  // 访问控制 / 路径兜底页：同为顶层全屏页，避免套上侧栏顶栏造成"带导航的报错页"
  {
    path: '/forbidden',
    name: 'Forbidden',
    component: () => import('../views/RouteFallback.vue'),
    meta: { public: true, title: '无权访问', fallbackKind: 'forbidden' },
  },
  {
    path: '/not-found',
    name: 'NotFound',
    component: () => import('../views/RouteFallback.vue'),
    meta: { public: true, title: '页面不存在', fallbackKind: 'not-found' },
  },
  {
    path: '/',
    component: () => import('../views/Layout.vue'),
    redirect: '/overview',
    children: pageRoutes,
  },
  {
    path: '/:pathMatch(.*)*',
    // 打错 / 已下线的地址不再静默跳仪表盘：落到 404 页并带上原路径，
    // 现场反馈"点了链接回到首页"时能直接看出是路径问题
    redirect: (to) => ({ name: 'NotFound', query: { from: to.fullPath } }),
  },
]

const router = createRouter({
  history: createWebHistory(),
  routes,
})

/**
 * 路由层 dynamic import 失败兜底
 * ─────────────────────────────────────────
 * Vue Router 对 dynamic import() reject 默认不兜底，导致整页空白。
 * 这里捕获两类典型错误（来自 vite/webpack 旧版 chunk 加载失败 / ESM 动态导入失败），
 * 跳转 /load-failed 占位页，让用户可以"重试"或"返回首页"恢复。
 *
 * 注意：只处理 chunk/import 错误。其他错误（如路由守卫 reject）由守卫自己负责。
 */
router.onError((err, to) => {
  const msg = err && (err as Error).message ? (err as Error).message : String(err)
  const isChunkOrImportError =
    /Loading chunk \d+ failed|ChunkLoadError|Failed to fetch dynamically imported module|Importing a module script failed/i.test(
      msg,
    )
  if (!isChunkOrImportError) {
    // 非动态导入错误：保留原行为（让 Vue Router 走自己的错误兜底）
    logError('router', err, '[router.onError]')
    return
  }
  // 避免在 LoadFailed 自身加载失败时再次触发死循环
  if (to?.name === 'LoadFailed') return
  // 携带错误信息跳转到兜底页
  router
    .replace({
      name: 'LoadFailed',
      query: { from: to?.fullPath ?? location.pathname + location.search },
    })
    .catch(() => {
      // replace 本身也可能失败（如 query 太长），降级为 location.assign
      window.location.assign('/load-failed')
    })
  // 同步上报 error reporter（页面级错误的 dashboard 指示器可见）
  // 注意：reportPageError 在 useErrorReporter 中提供
  // 这里用动态 import 避免循环依赖
  import('../composables/core/useErrorReporter')
    .then(({ reportPageError }) => {
      reportPageError({
        boundary: 'route-load',
        name: to?.fullPath ?? 'unknown',
        error: err,
        route: to?.fullPath,
        info: { kind: 'dynamic-import' },
      })
    })
    .catch(() => { /* ignore */ })
})

router.beforeEach(async (to, from) => {
  const auth = useAuthStore()

  // 公开路由
  if (to.meta.public) return true

  // 未保存更改守卫：仅拦 path 变化（Hub 内 query tab 切换由 HubShell before-leave 拦，避免双弹）
  // Bug-W6 (fix)：按 path 分桶脏标记——只询问/清空 from.path（当前离开的路由），
  // 其他 tab 的脏数据完整保留，避免误清。
  if (to.path !== from.path && isScopeDirty(from.path)) {
    const ok = await confirmDiscardDirty(from.path)
    if (!ok) return false
  }

  // 未登录：跳登录
  if (!auth.loggedIn) return '/login'

  // 已登录但本地未 whoami：路由切换时 ping 一次 whoami（仅 username 为空时）。
  // 说明：auth store 的 username 每次页面加载都从空串开始（不从 localStorage 恢复），
  // 因此刷新后必然走一次 validate()；validate() 会用真实可解密 token 校正 loggedIn，
  // 故无需在此额外依赖 authStatus。
  if (!auth.username) {
    const ok = await auth.validate()
    if (!ok) {
      auth.logout()
      return '/login'
    }
  }

  // v2.0：tier 守卫已移除——调试/分析/批量功能对所有 tier（含失效态）免费，
  // license 只约束产能配额（保存配置时后端返回 4021，由 http 拦截器统一提示）。

  // admin 守卫（meta 由 Vue Router 从 matched 记录合并，直接读 to.meta 即可）
  const needAdmin = to.meta.requireAdmin
  if (needAdmin && auth.roleId !== 'admin') {
    // 不再只弹一句一闪而过的 toast 后跳回仪表盘：给出可停留的说明页，
    // 并带上原路径，让"没权限"与"地址打错"（走 NotFound）两种失败可区分
    return { name: 'Forbidden', query: { from: to.fullPath } }
  }

  return true
})

export default router
