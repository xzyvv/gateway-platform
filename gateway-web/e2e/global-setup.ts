import { chromium, expect, type Page } from '@playwright/test'

/**
 * E2E 预热（在全部用例之前跑一次）
 *
 * 为什么需要：vite dev server 是按需发现依赖的。全套用例里**第一次**真正进入
 * `/overview` 时，vite 会发现一批此前没预打包的依赖，触发「重新预打包 + 整页
 * reload」。这次 reload 恰好发生在登录跳转过程中，会把会话冲回 /login，于是
 * 第一个用例（debug-paths.spec.ts「协议调试台」）稳定假红：日志里能看到
 * login/whoami/license/status 全是 200，随后 overview 的一串模块请求挂起，
 * 最终 URL 仍是 /login。
 *
 * 直接在 CI 里放宽超时治不了这个病（实测 30s 也等不到），因为它不是慢，
 * 而是被 reload 打断了。这里先用一个临时浏览器把「首次进入 /overview」
 * 消耗掉，用例跑起来时 optimizer 已经热了。
 *
 * 预热失败不阻断：用例自身还有 30s 断言余量，真错了会在用例里报出来。
 */

const BASE_URL = 'http://localhost:5199'
const USERNAME_PLACEHOLDER = '请输入用户名'
const PASSWORD_PLACEHOLDER = '请输入密码'

async function loginOnce(page: Page): Promise<void> {
  // 与用例保持一致：静音登录页的局域网探针（254×3 个地址，预热阶段没必要真扫）
  await page.route(/\/health\/live(?:[?#]|$)/, (route) => route.abort())
  await page.goto(`${BASE_URL}/login`)
  await page.getByPlaceholder(USERNAME_PLACEHOLDER).fill('admin')
  await page.getByPlaceholder(PASSWORD_PLACEHOLDER).fill('admin')
  await page.getByRole('button', { name: '登录' }).click()
  await expect(page).toHaveURL(/\/overview$/, { timeout: 30_000 })
}

export default async function globalSetup(): Promise<void> {
  const browser = await chromium.launch()
  try {
    const page = await browser.newPage()
    try {
      await loginOnce(page)
    } catch {
      // 第一次登录可能正好被 re-optimize 的 reload 打断（URL 被冲回 /login），
      // 第二次一定走已预打包的缓存。
      await loginOnce(page)
    }
  } catch {
    // 预热只是省时间，不影响正确性：真实失败交给用例自己暴露。
  } finally {
    await browser.close()
  }
}
