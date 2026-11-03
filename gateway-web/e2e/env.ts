/**
 * E2E 后端地址口径（P2-3）。
 *
 * 端口默认 18080，与 playwright.config.ts 中 webServer 的 GATEWAY_E2E_PORT 保持一致；
 * 用例里不要再硬编码 127.0.0.1:8080——那可能正连着现场真实网关，写用例会污染配置。
 */
export const GATEWAY_E2E_PORT = Number(process.env.GATEWAY_E2E_PORT ?? 18080)

export const GATEWAY_E2E_URL =
  process.env.GATEWAY_E2E_URL ?? `http://127.0.0.1:${GATEWAY_E2E_PORT}`
