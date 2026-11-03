import { defineConfig, devices } from '@playwright/test'

/**
 * Playwright E2E 冒烟配置
 * ───────────────────────────────────────────────────────────
 * 范围刻意克制：仅 chromium、串行（workers=1，对单一后端实例避免状态竞争）、
 * 6 条只读核心链路（见 e2e/smoke.spec.ts）。
 *
 * 服务编排（两个 webServer，测试结束自动回收）：
 *   1. scripts/e2e/start-gateway.mjs  起空配置 gatewayd（默认 :18080，见下方隔离说明）
 *   2. vite dev server（:5199，proxy /api 与 ^/health/ → E2E 网关）
 *
 * P2-3 环境隔离：E2E 后端**不复用** 8080。现场调试时 8080 上可能正跑着真实网关，
 * 旧配置 `reuseExistingServer: !CI` 会把冒烟与写用例直接打到真实设备上（配置被改、
 * 版本库被写）。现在：
 *   - 端口走 GATEWAY_E2E_PORT，默认 18080，且 reuseExistingServer 固定 false；
 *   - 端口被占用时直接失败，而不是"默默地连上别人的网关"；
 *   - vite 的 /api 代理通过 GATEWAY_API_TARGET 指向该端口，不再假设 8080。
 *
 * 防 flaky 约定：
 * - retries=0（重试是 flaky 的遮羞布；CI 观察期稳定后可给 CI 单独设 1）
 * - 失败保留 trace/screenshot/video，成功不保留（控 artifact 体积）
 * - 用例内禁止固定 sleep，只靠 web-first 断言自动等待
 */
const FRONTEND_PORT = 5199
// 独立端口：默认 18080，避开真实网关的 8080
const GATEWAY_PORT = Number(process.env.GATEWAY_E2E_PORT ?? 18080)
const GATEWAY_URL = `http://127.0.0.1:${GATEWAY_PORT}`
// 同一进程内的 spec 通过 e2e/env.ts 读取；在这里落默认值，保证 config 与用例口径一致
process.env.GATEWAY_E2E_PORT ??= String(GATEWAY_PORT)

export default defineConfig({
  testDir: './e2e',
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  reporter: process.env.CI ? [['list'], ['html', { open: 'never' }]] : [['list'], ['html', { open: 'on-failure' }]],
  // 单用例 60s：首个用例要替全量用例付 vite 冷启动（按需预打包 element-plus），
  // 登录往返实测可达 ~19s，30s 会把冷启动那条挤到超时边缘。
  timeout: 60_000,
  expect: { timeout: 10_000 },

  use: {
    baseURL: `http://localhost:${FRONTEND_PORT}`,
    trace: 'retain-on-failure',
    screenshot: 'on-first-failure',
    video: 'retain-on-failure',
    actionTimeout: 10_000,
    locale: 'zh-CN',
  },

  projects: [
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],

  webServer: [
    {
      // 空配置网关：脚本自带就绪轮询与退出清理。
      // 固定 false：8080/18080 上若已有真实网关，宁可失败也不要污染现场配置。
      command: 'node scripts/e2e/start-gateway.mjs',
      url: `${GATEWAY_URL}/health/live`,
      reuseExistingServer: false,
      timeout: 60_000,
      env: {
        GATEWAY_E2E_PORT: String(GATEWAY_PORT),
        GATEWAY_HEALTH_URL: `${GATEWAY_URL}/health/live`,
      },
    },
    {
      command: `npx vite --port ${FRONTEND_PORT} --strictPort`,
      url: `http://localhost:${FRONTEND_PORT}/`,
      // 同样固定 false：复用已在跑的 5199 dev server 会绕过上面的代理指向，
      // 变成连到它自己配置的（可能是 8080 真实）网关。
      reuseExistingServer: false,
      timeout: 60_000,
      // 代理指向隔离的 E2E 网关，避免落到本机 8080 的真实设备
      env: {
        GATEWAY_API_TARGET: GATEWAY_URL,
      },
    },
  ],
})
