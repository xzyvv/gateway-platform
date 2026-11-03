import { describe, it, expect, vi } from 'vitest'
import { createRouter, createMemoryHistory } from 'vue-router'

/** router.onError 兜底测试
 *
 * 直接构造一个 createRouter 实例，传入同样的 onError 逻辑（与 src/router/index.ts 一致），
 * 验证错误识别与跳转行为。完整路由 index.ts 集成测试在浏览器/E2E 层做。
 */

const mockReportPageError = vi.fn()
vi.mock('../composables/core/useErrorReporter', () => ({
  reportPageError: (...args: unknown[]) => mockReportPageError(...args),
}))

/** 测试用精简 handler 签名：只声明 onError 逻辑实际用到的字段 */
type ErrorHandler = (err: Error, to?: { name?: unknown; fullPath?: string }) => void

// 从 index.ts 复制 onError 逻辑（避免模块副作用影响其他测试）；返回注册的 handler 供测试直接触发
function installOnError(router: ReturnType<typeof createRouter>): ErrorHandler {
  const handler: ErrorHandler = (err, to) => {
    const msg = err && (err as Error).message ? (err as Error).message : String(err)
    const isChunkOrImportError =
      /Loading chunk \d+ failed|ChunkLoadError|Failed to fetch dynamically imported module|Importing a module script failed/i.test(
        msg,
      )
    if (!isChunkOrImportError) {
      console.error('[router.onError]', err)
      return
    }
    if (to?.name === 'LoadFailed') return
    router.replace({
      name: 'LoadFailed',
      query: { from: to?.fullPath ?? '' },
    }).catch(() => {})
    mockReportPageError({
      boundary: 'route-load',
      name: to?.fullPath ?? 'unknown',
      error: err,
      route: to?.fullPath,
      info: { kind: 'dynamic-import' },
    })
  }
  router.onError(handler)
  return handler
}

// 记录 makeRouter 时经公开 API router.onError 注册的 handler，供测试直接触发
let installedHandler: ErrorHandler | null = null

function makeRouter() {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/dashboard', name: 'Dashboard', component: { template: '<div />' } },
      {
        path: '/load-failed',
        name: 'LoadFailed',
        component: { template: '<div />' },
      },
      { path: '/devices', name: 'Devices', component: { template: '<div />' } },
    ],
  })
  installedHandler = installOnError(router)
  return router
}

/** 直接触发经 router.onError 注册的 handler（jsdom 不真正执行 dynamic import） */
async function triggerError(msg: string) {
  if (!installedHandler) return false
  installedHandler(new Error(msg), { name: 'Devices', fullPath: '/devices' })
  return true
}

describe('router.onError', () => {
  it('chunk 错误触发跳转到 /load-failed 并携带 from query', async () => {
    const router = makeRouter()
    await router.push('/dashboard')
    await router.isReady()

    mockReportPageError.mockClear()
    // 直接触发 router.onError 注册的 handler（jsdom 不真正执行 dynamic import）
    await triggerError('Loading chunk 5 failed. (error: /assets/Page-xyz.js)')
    await new Promise((r) => setTimeout(r, 50))
    expect(router.currentRoute.value.name).toBe('LoadFailed')
    expect(router.currentRoute.value.query.from).toBe('/devices')
    expect(mockReportPageError).toHaveBeenCalled()
  })

  it('非 chunk 错误（守卫 reject 等）不被误判为动态加载错误', () => {
    const re =
      /Loading chunk \d+ failed|ChunkLoadError|Failed to fetch dynamically imported module|Importing a module script failed/i
    expect(re.test('Navigation aborted')).toBe(false)
    expect(re.test('Some other error')).toBe(false)
  })

  it('正则匹配所有已知变体', () => {
    const re =
      /Loading chunk \d+ failed|ChunkLoadError|Failed to fetch dynamically imported module|Importing a module script failed/i
    expect(re.test('Loading chunk 0 failed')).toBe(true)
    expect(re.test('ChunkLoadError: Loading chunk 1 failed')).toBe(true)
    expect(re.test('Failed to fetch dynamically imported module: /assets/x.js')).toBe(true)
    expect(re.test('Importing a module script failed')).toBe(true)
  })
})