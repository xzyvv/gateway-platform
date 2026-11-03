import { test, expect, type Page } from '@playwright/test'
import { loginViaForm, stubLanDiscovery } from './helpers'

/**
 * E2E 阶段 2：全量路由巡游
 * ───────────────────────────────────────────────────────────
 * 与 smoke.spec.ts 的分工（刻意不合并）：
 *   - smoke：5 条核心链路，含严格的"零 console 错误"断言，覆盖面小但门槛高
 *   - 本文件：把巡游扩到全部高风险页面，按业务域拆 4 条用例，失败时能直接
 *     定位到是哪一组
 *
 * N-13：原「错误提示口径」两条用例（写静默 / 读全局报错）已由 vitest
 * `src/api/http-error-silent.spec.ts` 完全覆盖，此处删除，避免 e2e 与单测双跑。
 *
 * 与 smoke 一样的约定：
 * - 后端为空配置 gateway-core（无总线、无点位、无 MQTT），本文件全部为只读操作
 * - 用例内禁止固定 sleep，只靠 web-first 断言自动等待
 * - 定位优先 role / 可见文本；`.main-content`、`.page-error-boundary` 仅用于
 *   白屏与错误边界的存在性检测
 *
 * 为什么本文件不照搬 smoke 的"零 console 错误"断言：
 * 页面里有大量接口在空配置网关下返回 404/500，个别页面会在自己的 catch
 * 分支里 console.error。这类噪音只说明"空配置下该功能不可用"，不是渲染回归；
 * 真正要拦住的是未捕获异常（pageerror）与白屏，故此处只对 pageerror 硬断言。
 */

interface TourRoute {
  path: string
  /** 落地 URL（Hub 页可能带 ?tab= query；旧地址会被重定向到合并后的 Hub） */
  finalUrl: RegExp
  /** document.title 应包含的页面标题（来自 route.meta.title） */
  docTitle: string
}

/**
 * 巡游页面：覆盖概览、设备、点位、采集、数据、配置、系统、上行、运维。
 * 标题与路径以 src/router/routes/index.ts 的 meta.title 为准，路由调整需同步。
 * 路径语义 = `/<业务域>[/<视图能力>]`，Hub 页的 tab 通过 `?tab=` 表达。
 *
 * 第 1 期冗余收口后，同域 Hub 已合并（16 → 10），旧地址（/devices/ops、
 * /points/address-tools、/data/quality、/ops/perf|comm|health、/system/logs）
 * 保留为重定向——这里刻意把它们一起巡游，用于守住「旧书签仍可用」。
 */
const GROUP_CORE: TourRoute[] = [
  { path: '/overview', finalUrl: /\/overview$/, docTitle: '运行总览' },
  { path: '/overview/realtime', finalUrl: /\/overview\/realtime$/, docTitle: '实时数据' },
  { path: '/devices', finalUrl: /\/devices(\?.*)?$/, docTitle: '设备管理' },
  // 旧「设备运维」Hub → 设备管理 › 设备健康评分
  { path: '/devices/ops', finalUrl: /\/devices\?tab=health-score$/, docTitle: '设备管理' },
  { path: '/devices/topology', finalUrl: /\/devices\/topology$/, docTitle: '设备拓扑' },
]

const GROUP_DATA: TourRoute[] = [
  { path: '/points', finalUrl: /\/points(\?.*)?$/, docTitle: '点位管理' },
  // 旧「地址换算」Hub → 点位管理 › 地址换算
  { path: '/points/address-tools', finalUrl: /\/points\?tab=addr-calculator$/, docTitle: '点位管理' },
  { path: '/acquisition/tester', finalUrl: /\/acquisition\/tester(\?.*)?$/, docTitle: '点位读写' },
  { path: '/acquisition/console', finalUrl: /\/acquisition\/console(\?.*)?$/, docTitle: '协议调试台' },
  { path: '/acquisition/comm-tuning', finalUrl: /\/acquisition\/comm-tuning(\?.*)?$/, docTitle: '采集调优' },
  { path: '/data', finalUrl: /\/data(\?.*)?$/, docTitle: '数据流水' },
]

const GROUP_CONFIG_SYSTEM: TourRoute[] = [
  { path: '/system/config', finalUrl: /\/system\/config(\?.*)?$/, docTitle: '配置管理' },
  { path: '/system/config?tab=runtime-config', finalUrl: /\/system\/config(\?.*)?$/, docTitle: '配置管理' },
  { path: '/system', finalUrl: /\/system(\?.*)?$/, docTitle: '系统设置' },
  // 旧「运行日志」Hub → 系统设置 › 实时日志
  { path: '/system/logs', finalUrl: /\/system\?tab=logs$/, docTitle: '系统设置' },
  // 旧「数据质量」Hub → 数据流水 › 质量概览
  { path: '/data/quality', finalUrl: /\/data\?tab=data-quality$/, docTitle: '数据流水' },
  { path: '/uplink', finalUrl: /\/uplink(\?.*)?$/, docTitle: '上行转发' },
]

const GROUP_OPS: TourRoute[] = [
  { path: '/ops', finalUrl: /\/ops(\?.*)?$/, docTitle: '运维监控' },
  // 旧「性能监控 / 链路诊断 / 健康巡检」三个 Hub → 运维监控 对应页签
  { path: '/ops/perf', finalUrl: /\/ops\?tab=bus-load$/, docTitle: '运维监控' },
  { path: '/ops/comm', finalUrl: /\/ops\?tab=connection-test$/, docTitle: '运维监控' },
  { path: '/ops/health', finalUrl: /\/ops\?tab=gateway-diag$/, docTitle: '运维监控' },
]

test.beforeEach(async ({ page }) => {
  // 与 smoke 一致：登录页挂载即触发局域网发现，E2E 环境无意义且会拖慢按钮稳定性。
  await stubLanDiscovery(page)
})

/** 未捕获异常收集（pageerror 才是渲染回归的硬信号） */
function trackPageErrors(page: Page): string[] {
  const pageErrors: string[] = []
  page.on('pageerror', (err) => pageErrors.push(err.message))
  return pageErrors
}

/** 逐页断言：重定向正确、标题正确、未降级、未白屏 */
async function assertRouteRendered(page: Page, route: TourRoute): Promise<void> {
  await page.goto(route.path)

  await expect(page).toHaveURL(route.finalUrl)

  await expect
    .poll(() => page.title(), { message: `${route.path} 的 document.title 未包含页面标题` })
    .toContain(route.docTitle)

  // 错误边界仅在捕获到渲染错误后才出现在 DOM
  await expect(page.locator('.page-error-boundary')).toHaveCount(0)

  await expect(page.locator('.main-content')).toBeVisible()
  await expect
    .poll(async () => (await page.locator('.main-content').innerText()).trim().length, {
      message: `${route.path} 主内容区无实质文本（疑似白屏）`,
    })
    .toBeGreaterThan(20)
}

async function tourGroup(page: Page, routes: TourRoute[]): Promise<void> {
  await loginViaForm(page)
  const pageErrors = trackPageErrors(page)

  for (const route of routes) {
    await assertRouteRendered(page, route)
  }

  expect(pageErrors, `未捕获异常: ${JSON.stringify(pageErrors)}`).toEqual([])
}

test.describe('gateway-web E2E 路由巡游（21 页）', () => {
  test('概览与设备域（5 页）', async ({ page }) => {
    await tourGroup(page, GROUP_CORE)
  })

  test('点位、采集与数据域（6 页）', async ({ page }) => {
    await tourGroup(page, GROUP_DATA)
  })

  test('配置、系统与上行域（6 页）', async ({ page }) => {
    await tourGroup(page, GROUP_CONFIG_SYSTEM)
  })

  test('运维域与旧地址重定向（4 页）', async ({ page }) => {
    await tourGroup(page, GROUP_OPS)
  })
})
