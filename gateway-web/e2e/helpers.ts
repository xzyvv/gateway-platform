import { expect, type Page } from '@playwright/test'

/**
 * E2E 共用辅助
 *
 * 登录流程与"局域网扫描静音"这两个前置步骤必须由所有 spec 共享同一份实现：
 * 两份拷贝一旦出现细微差异（比如一边等扫描结束、另一边没等），就会出现
 * 一个文件稳定通过、另一个文件偶发失败的假绿/假红，排查成本远高于抽出来。
 */

/** 登录页可见文案（与 src/views/auth/Login.vue 核实一致，改动需同步） */
export const LOGIN_USERNAME_PLACEHOLDER = '请输入用户名'
export const LOGIN_PASSWORD_PLACEHOLDER = '请输入密码'

/**
 * 让登录页的局域网网关发现立即失败。
 *
 * 登录页挂载即向 254×3 个地址发探针，E2E 环境无意义，且有两个副作用：
 * 扫描区 DOM 伸缩导致登录按钮持续 not stable；探针产生网络/CORS 控制台噪音。
 * 健康徽章走的是 /health/ready，不受本拦截影响。
 */
export async function stubLanDiscovery(page: Page): Promise<void> {
  await page.route(/\/health\/live(?:[?#]|$)/, (route) => route.abort())
}

/**
 * admin/admin 登录并等待落地 /overview。
 * 浏览器（非 Tauri）场景无需填写网关地址，请求经 vite proxy 直达隔离的 E2E 网关（默认 :18080）。
 */
export async function loginViaForm(page: Page): Promise<void> {
  await page.goto('/login')

  // 等局域网扫描彻底结束（探针已被 stubLanDiscovery 快速失败，正常毫秒级返回；
  // 兜底 15s 覆盖 WebRTC 探测最坏路径），避免扫描区伸缩期间按钮位置抖动。
  await expect(page.getByText(/正在扫描同网段网关/)).toBeHidden({ timeout: 15_000 })

  await page.getByPlaceholder(LOGIN_USERNAME_PLACEHOLDER).fill('admin')
  await page.getByPlaceholder(LOGIN_PASSWORD_PLACEHOLDER).fill('admin')
  await page.getByRole('button', { name: '登录' }).click()
  // CI 首次导航要等 vite 按需预打包 element-plus（实测首个用例登录往返可到 ~19s，
  // 后续用例 ~6s）。10s 的默认断言超时会在冷启动那一条上假红，故单独放宽。
  await expect(page).toHaveURL(/\/overview$/, { timeout: 30_000 })
}
