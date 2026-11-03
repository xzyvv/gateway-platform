import { test, expect, type Page } from '@playwright/test'
import { loginViaForm, stubLanDiscovery } from './helpers'
import { GATEWAY_E2E_URL } from './env'

/**
 * E2E 冒烟（阶段 1：本地跑通）
 * ───────────────────────────────────────────────────────────
 * 刻意克制的 5 条只读核心链路：
 *   G0 栈健康 → 登录核心链路 → 核心路由巡游
 *   → license 状态端点 → 登出
 *
 * N-13：暗色持久化与 404/降级路由两例已由 vitest（theme.spec / LoadFailedRoute.spec /
 * routerError.spec）完全覆盖，e2e 只保留「真实浏览器 + 真实后端」才能验证的链路。
 *
 * 约定：
 * - 后端为空配置 gateway-core（无总线/点位/MQTT，不做任何写操作、串口、OTA）
 * - Playwright 默认每条用例获得全新 BrowserContext：localStorage/IndexedDB/cookie
 *   天然隔离，用例间无登录态污染，无需手工清理
 * - 禁止固定 sleep：全部依赖 web-first 断言（toBeVisible/toHaveURL/toHaveText 等）
 *   的自动等待能力
 * - 定位优先 role / placeholder / 可见文本，不依赖 CSS class 与 DOM 层级
 *   （个别 scoped 根容器 class 仅用于"白屏检测"的存在性断言）
 *
 * License（§13 E2E）说明：dev server 下 import.meta.env.DEV=true，tier 语义为
 * 开发模拟（手动切档、菜单全开），生产档位降级与徽章属生产构建行为，无法在
 * dev server 上断言；此处验证真实后端的 /api/license 链路（absent → core 档）。
 * 导入/过期 license 场景需生产私钥签发（网关公钥为编译期常量），记入设计文档开放项。
 */

/**
 * 巡游路由：访问路径 → 最终落地 URL（路径语义 = `/<业务域>[/<视图能力>]`）
 * 与路由 afterEach 应写入的 document.title。
 * title 来自 route.meta.title（routes/index.ts 中直接写在 meta 里，
 * main.ts afterEach 据此设置标签页标题）。
 */
const TOUR_ROUTES = [
  { path: '/overview', finalUrl: /\/overview$/, docTitle: '运行总览' },
  { path: '/devices', finalUrl: /\/devices(\?.*)?$/, docTitle: '设备管理' },
  { path: '/system/config?tab=runtime-config', finalUrl: /\/system\/config(\?.*)?$/, docTitle: '配置管理' },
  { path: '/data', finalUrl: /\/data(\?.*)?$/, docTitle: '数据流水' },
  { path: '/system', finalUrl: /\/system$/, docTitle: '系统设置' },
] as const

test.beforeEach(async ({ page }) => {
  // 登录页挂载即触发局域网发现，E2E 环境无意义且会拖慢按钮稳定性；
  // 统一让探针立即失败，详见 helpers.stubLanDiscovery。
  await stubLanDiscovery(page)
})

/**
 * 收集页面运行期问题（用于路由巡游断言"无白屏、无降级、无报错"）。
 *
 * 过滤说明：登录页挂载时会触发局域网网关发现（useGatewayDiscovery），
 * 对不可达 IP 的探测在个别环境下可能在控制台留下网络/CORS 类噪音，
 * 与应用自身质量无关，按关键词忽略；未捕获异常（pageerror）一律算失败。
 */
function trackPageIssues(page: Page): { consoleErrors: string[]; pageErrors: string[] } {
  const consoleErrors: string[] = []
  const pageErrors: string[] = []
  const benignPattern = /health\/live|CORS|cross-origin|net::ERR_|Failed to fetch|NetworkError|load failed/i
  page.on('console', (msg) => {
    if (msg.type() !== 'error') return
    const text = msg.text()
    if (benignPattern.test(text)) return
    consoleErrors.push(text)
  })
  page.on('pageerror', (err) => {
    pageErrors.push(err.message)
  })
  return { consoleErrors, pageErrors }
}

test.describe('gateway-web E2E 冒烟', () => {
  test('G0 栈健康：gateway-core 隔离实例与前端 dev server 均可达', async ({ page, request }) => {
    // 后端直连探针（不经 vite proxy，验证夹具本身健康）；端口来自 e2e/env.ts，
    // 默认 18080，避免误连现场 8080 的真实网关
    const resp = await request.get(`${GATEWAY_E2E_URL}/health/live`)
    expect(resp.status()).toBe(200)
    expect((await resp.json()).status).toBe('ok')

    // 前端首页：未登录访问 / 应由路由守卫弹回 /login，且登录卡片正常渲染
    await page.goto('/')
    await expect(page).toHaveURL(/\/login$/)
    await expect(page.getByRole('heading', { name: '工业网关', level: 1 })).toBeVisible()
  })

  test('登录核心链路：admin/admin 登录成功、健康徽章正常、刷新保持登录', async ({ page }) => {
    await loginViaForm(page)

    // 空配置下 /health/ready 全部组件 ok，徽章应从"检测中..."翻为「服务在线」
    // （P0-5：正常态文案由「网关正常」改为「服务在线」，与后端可达而非设备全好同义）
    await expect(page.locator('.health-badge')).toHaveText('服务在线')

    // token 持久化：整页刷新后路由守卫 whoami 校验通过，仍在 overview
    await page.reload()
    await expect(page).toHaveURL(/\/overview$/)
    await expect(page.locator('.health-badge')).toHaveText('服务在线')
  })

  test('核心路由巡游：5 个核心页面无白屏、无错误降级、无 console/page 错误', async ({ page }) => {
    await loginViaForm(page)
    const issues = trackPageIssues(page)

    for (const route of TOUR_ROUTES) {
      await page.goto(route.path)

      // 落地 URL 与路由语义一致（Hub 页可能带 ?tab= query）
      await expect(page).toHaveURL(route.finalUrl)

      // document.title 来自 meta.title（main.ts afterEach 可读到）
      await expect
        .poll(() => page.title(), { message: `${route.path} 的 document.title 未包含页面标题` })
        .toContain(route.docTitle)

      // 页面级错误边界未降级（.page-error-boundary 仅在捕获错误后才出现在 DOM）
      await expect(page.locator('.page-error-boundary')).toHaveCount(0)

      // 白屏检测：主内容区可见且渲染出实质文本（expect.poll 自动重试，无需固定等待）
      await expect(page.locator('.main-content')).toBeVisible()
      await expect
        .poll(async () => (await page.locator('.main-content').innerText()).trim().length, {
          message: `${route.path} 主内容区无实质文本（疑似白屏）`,
        })
        .toBeGreaterThan(20)
    }

    expect(issues.pageErrors, `未捕获异常: ${JSON.stringify(issues.pageErrors)}`).toEqual([])
    expect(issues.consoleErrors, `控制台错误: ${JSON.stringify(issues.consoleErrors)}`).toEqual([])
  })

  test('license 状态端点：空配置网关返回 absent / core 档 / ok', async ({ request }) => {
    // 真实后端链路（§13 E2E-1 的 API 断言部分）：无 license → core 档基线。
    // /api/license 属「敏感读」：即使 allow_anon_read=true 也强制鉴权
    // （见 gateway-api/src/web/middleware/auth.rs 的 is_sensitive_read_path），
    // 因此这里先登录再带 token，不能再依赖匿名读。
    const login = await request.post('/api/auth/login', {
      data: { username: 'admin', password: 'admin' },
    })
    expect(login.ok()).toBeTruthy()
    const { token } = (await login.json()) as { token: string }
    const resp = await request.get(`${GATEWAY_E2E_URL}/api/license`, {
      headers: { Authorization: `Bearer ${token}` },
    })
    expect(resp.status()).toBe(200)
    // /api/license 走 ApiResponse 信封（code/data/message），需解包 .data
    const envelope = (await resp.json()) as { code: number; data: Record<string, unknown> }
    expect(envelope.code).toBe(0)
    const body = envelope.data
    expect(body.present).toBe(false)
    expect(body.tier).toBe('core')
    expect(body.status).toBe('ok')
    expect(body.license_id).toBeNull()
    // v2.0 授权模型用 quota 取代了 v1.2 的 features 细粒度开关（见 api/generated/Quota.ts）
    expect(body).not.toHaveProperty('features')
    const quota = body.quota as Record<string, unknown>
    for (const key of ['max_points', 'max_protocols', 'storage_days', 'northbound_channels']) {
      expect(quota, `quota 缺少字段 ${key}`).toHaveProperty(key)
      const v = quota[key]
      expect(v === null || typeof v === 'number', `quota.${key} 应为 number|null，实得 ${JSON.stringify(v)}`).toBe(true)
    }
  })

  test('登出：确认后回登录页，受保护路由不可再访问', async ({ page }) => {
    await loginViaForm(page)

    // 顶栏用户菜单（按钮文本为用户名）→ "退出登录" 项 → 二次确认弹窗点"确定"
    await page.locator('.logout-btn').click()
    await page.getByText('退出登录', { exact: true }).click()
    await page.getByRole('button', { name: '确定' }).click()

    await expect(page).toHaveURL(/\/login$/)

    // 登出后直接访问受保护页，守卫必须弹回 /login
    await page.goto('/overview')
    await expect(page).toHaveURL(/\/login$/)
  })
})
