/**
 * 路由装配与守卫测试（src/router/index.ts）
 *
 * 覆盖 beforeEach 守卫的关键分支：公开路由放行、未登录跳登录、已登录但
 * whoami（validate）失败时登出、requireAdmin 拦截、未保存更改拦截。
 *
 * 说明：index.ts 在模块加载时会 createWebHistory() 并注册全局守卫（单例副作用），
 * 这里 mock 掉 Layout / 顶层视图组件与 ./routes，避免加载真实业务页面（重依赖）。
 * onError 兜底逻辑由同目录的 routerError.spec.ts 覆盖。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

vi.mock('../views/Layout.vue', () => ({
  default: { name: 'Layout', template: '<div><router-view /></div>' },
}))
vi.mock('../views/auth/Login.vue', () => ({
  default: { name: 'Login', template: '<div>login</div>' },
}))
vi.mock('../views/auth/ChangePassword.vue', () => ({
  default: { name: 'ChangePassword', template: '<div>cp</div>' },
}))
vi.mock('../views/LoadFailedRoute.vue', () => ({
  default: { name: 'LoadFailedRoute', template: '<div>load-failed</div>' },
}))

// 用轻量路由替代真实页面路由表（真实表另有 redirects.spec.ts 做一致性校验）
vi.mock('./routes', () => ({
  routes: [
    { path: '/dashboard', name: 'Dashboard', component: { template: '<div>dash</div>' }, meta: { title: '仪表盘' } },
    { path: '/plain', name: 'Plain', component: { template: '<div>plain</div>' }, meta: { title: '普通页' } },
    {
      path: '/admin-only',
      name: 'AdminOnly',
      component: { template: '<div>admin</div>' },
      meta: { title: '管理页', requireAdmin: true },
    },
  ],
}))

vi.mock('../utils/dirtyGuard', () => ({
  isAnyDirty: vi.fn(() => false),
  isScopeDirty: vi.fn(() => false),
  confirmDiscardDirty: vi.fn(async () => true),
}))

vi.mock('../composables/core/useErrorReporter', () => ({ reportPageError: vi.fn() }))

import router from './index'
import { useAuthStore } from '../stores/auth'
import { isScopeDirty, confirmDiscardDirty } from '../utils/dirtyGuard'

describe('router 守卫', () => {
  beforeEach(async () => {
    setActivePinia(createPinia())
    localStorage.clear()
    vi.mocked(isScopeDirty).mockReturnValue(false)
    vi.mocked(confirmDiscardDirty).mockResolvedValue(true)
    await router.replace('/login')
  })

  it('公开路由直接放行', async () => {
    await router.push('/load-failed')
    expect(router.currentRoute.value.path).toBe('/load-failed')
  })

  it('未登录访问受保护页面时跳转登录页', async () => {
    const auth = useAuthStore()
    auth.loggedIn = false
    await router.push('/plain')
    expect(router.currentRoute.value.path).toBe('/login')
  })

  it('已登录但本地无身份（validate 失败）时登出并跳登录页', async () => {
    const auth = useAuthStore()
    auth.loggedIn = true
    auth.username = ''
    vi.spyOn(auth, 'validate').mockResolvedValue(false)
    const logoutSpy = vi.spyOn(auth, 'logout')

    await router.push('/plain')

    expect(logoutSpy).toHaveBeenCalled()
    expect(router.currentRoute.value.path).toBe('/login')
  })

  it('已登录且身份有效时放行普通页面', async () => {
    const auth = useAuthStore()
    auth.loggedIn = true
    auth.username = 'admin'
    await router.push('/plain')
    expect(router.currentRoute.value.path).toBe('/plain')
  })

  it('非管理员访问 requireAdmin 页面时落到 403 页并带上原路径', async () => {
    const auth = useAuthStore()
    auth.loggedIn = true
    auth.username = 'op'
    auth.roleId = 'operator'

    await router.push('/admin-only')

    // 不再静默跳仪表盘：落到可停留的说明页，query.from 供排查时定位原路径
    expect(router.currentRoute.value.path).toBe('/forbidden')
    expect(router.currentRoute.value.query.from).toBe('/admin-only')
  })

  it('存在未保存更改且用户取消时阻断跳转', async () => {
    const auth = useAuthStore()
    auth.loggedIn = true
    auth.username = 'admin'
    vi.mocked(isScopeDirty).mockReturnValue(true)
    vi.mocked(confirmDiscardDirty).mockResolvedValue(false)

    await router.push('/plain').catch(() => {})

    expect(router.currentRoute.value.path).not.toBe('/plain')
  })
})