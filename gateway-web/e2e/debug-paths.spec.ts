import { test, expect, type Route } from '@playwright/test'
import { loginViaForm, stubLanDiscovery } from './helpers'

/**
 * 现场调试主路径 E2E（C1.2）
 *
 * 三条链路覆盖「现场工程师最常用的调试动作」：
 *   1. 协议调试台：总线协议能力位决定从站扫描能否点击（能力位路由的前端口径）；
 *   2. 采集调优：改总线超时/重试 → 自动保存并热重载（改参必须真的写回配置）；
 *   3. 点位读写：选设备/点位 → 读取回显 → 写入走二次确认 → 结果回显。
 *
 * 与 smoke.spec.ts 的只读巡游不同，本文件对业务接口做 page.route 拦截：
 * E2E 网关是空配置只读环境，写入会污染后续用例；这里验证的是「前端编排是否
 * 正确发起请求、正确展示结果」，请求是否真实落盘由后端集成测试与真实写链路
 * 用例（write-flows.spec.ts）各自保证。
 */

async function fulfillJson(route: Route, body: unknown, status = 200): Promise<void> {
  await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) })
}

/** 协议能力位默认全 false，按用例声明需要打开的位 */
function caps(over: Record<string, boolean> = {}): Record<string, boolean> {
  return {
    supports_batch_read: false,
    supports_frame_extract: false,
    supports_write: false,
    supports_scan_slaves: false,
    supports_register_scan: false,
    supports_auto_detect: false,
    supports_write_multi: false,
    supports_diag: false,
    ...over,
  }
}

test.beforeEach(async ({ page }) => {
  await stubLanDiscovery(page)
})

test('协议调试台：能力位不支持时禁用从站扫描，切到 Modbus 后放行', async ({ page }) => {
  const config = {
    buses: [
      { port_name: 'bus-iec', protocol: 'iec104', timeout_ms: 1000, retry_count: 0, retry_delay_ms: 100 },
      { port_name: 'bus-mb', protocol: 'modbus', timeout_ms: 1000, retry_count: 0, retry_delay_ms: 100 },
    ],
    devices: [],
    points: [],
  }

  await page.route('**/api/config', (route) => fulfillJson(route, config))
  await page.route('**/api/protocols', (route) =>
    fulfillJson(route, [
      { name: 'iec104', capabilities: caps() },
      { name: 'modbus', capabilities: caps({ supports_scan_slaves: true }) },
    ]),
  )

  await loginViaForm(page)
  await page.goto('/acquisition/console?tab=slave-scan')

  // 默认选中第一条总线（iec104）：入口必须禁用并说明「不适用」
  const scanButton = page.getByRole('button', { name: '扫描', exact: true })
  await expect(
    page.getByText('当前总线协议（iec104）不支持从站扫描'),
  ).toBeVisible()
  await expect(scanButton).toBeDisabled()
  await expect(
    page.getByText('从站扫描不适用（不是尚未实现）'),
  ).toBeVisible()

  // 切到 modbus 总线：能力位放行，提示消失
  await page.locator('.ss .el-select__wrapper').first().click()
  await page.getByRole('option', { name: 'bus-mb (modbus)' }).click()

  await expect(
    page.getByText('当前总线协议（iec104）不支持从站扫描'),
  ).toBeHidden()
  await expect(scanButton).toBeEnabled()
})

test('采集调优：改总线超时后自动保存，请求体带新值与其它总线原值', async ({ page }) => {
  const config = {
    buses: [
      { port_name: 'bus-1', protocol: 'modbus', timeout_ms: 1000, retry_count: 2, retry_delay_ms: 500 },
      { port_name: 'bus-2', protocol: 'modbus', timeout_ms: 2000, retry_count: 1, retry_delay_ms: 300 },
    ],
    devices: [
      { id: 'dev-1', name: '设备1', bus: 'bus-1', protocol: 'modbus', address: '1', slave_id: 1, interval: 5 },
    ],
    points: [],
  }
  const captured: { body?: { buses?: Array<Record<string, unknown>> } } = {}

  await page.route('**/api/config', async (route) => {
    if (route.request().method() === 'GET') return fulfillJson(route, config)
    captured.body = route.request().postDataJSON() as typeof captured.body
    await fulfillJson(route, { status: 'ok', applied: ['buses'], restart_required: [] })
  })

  await loginViaForm(page)
  await page.goto('/acquisition/comm-tuning?tab=comm-params')

  // 总线行：第一条是 bus-1（超时输入框即该行第一个 spinbutton）
  await expect(page.getByRole('cell', { name: 'bus-1', exact: true }).first()).toBeVisible()
  const timeoutInput = page.getByRole('spinbutton').first()
  await expect(timeoutInput).toHaveValue('1000')

  await timeoutInput.fill('1500')
  await timeoutInput.press('Tab')

  await expect(page.getByText('已保存总线 bus-1 的参数（影响 1 台设备）')).toBeVisible()
  expect(captured.body?.buses?.[0]).toMatchObject({ port_name: 'bus-1', timeout_ms: 1500 })
  // 保存必须提交完整配置：未被编辑的 bus-2 保持原值
  expect(captured.body?.buses?.[1]).toMatchObject({ port_name: 'bus-2', timeout_ms: 2000 })
})

test('点位读写：读取回显 → 写入二次确认 → 写入结果回显', async ({ page }) => {
  const device = {
    id: 'dev-1',
    name: '现场从站',
    template: 'modbus-holding',
    bus: 'bus-1',
    protocol: 'modbus',
    address: '1',
    slave_id: 1,
    interval: 5,
  }
  const point = {
    id: 'p-1',
    name: '温度',
    device_id: 'dev-1',
    bus: 'bus-1',
    protocol: 'modbus',
    address: '40001',
    slave_id: 1,
    interval: 5,
    data_type: 'int16',
    writable: true,
  }
  let reads = 0
  const captured: { body?: { writes?: Array<{ id: string; value: number }> } } = {}

  await page.route('**/api/devices', (route) => fulfillJson(route, [device]))
  await page.route('**/api/devices/dev-1/points', (route) => fulfillJson(route, [point]))
  await page.route('**/api/protocols', (route) =>
    fulfillJson(route, [
      {
        name: 'modbus',
        capabilities: caps({ supports_write: true, supports_batch_read: true, supports_scan_slaves: true, supports_diag: true }),
      },
    ]),
  )
  await page.route('**/api/protocol-tools/packet-capture/packets*', (route) => fulfillJson(route, []))
  await page.route('**/api/points/p-1/read', async (route) => {
    reads += 1
    await fulfillJson(route, {
      status: 'ok',
      value: reads === 1 ? 25.5 : 26.5,
      quality: 'Good',
    })
  })
  await page.route('**/api/point-tools/write', async (route) => {
    captured.body = route.request().postDataJSON() as typeof captured.body
    await fulfillJson(route, {
      status: 'ok',
      total: 1,
      succeeded: 1,
      failed: 0,
      results: [{ point_id: 'p-1', status: 'ok', value: 26.5, quality: 'Good' }],
    })
  })

  await loginViaForm(page)
  await page.goto('/acquisition/tester')

  // 选设备 → 选点位（能力徽标随协议声明出现）
  await page.locator('.el-select__wrapper').nth(0).click()
  await page.getByRole('option', { name: '现场从站 (modbus)' }).click()
  await expect(page.getByText('支持写入', { exact: true })).toBeVisible()

  const pointSelect = page.locator('.el-select__wrapper').nth(1)
  await pointSelect.click()
  await page.getByRole('option', { name: '温度 (int16)' }).click()

  // 读取：当前值与操作记录都回显
  await page.getByRole('button', { name: '读取', exact: true }).click()
  await expect(page.locator('.value-number')).toContainText('25.5')
  await expect(page.getByText(/温度 = 25\.5/)).toBeVisible()

  // 写入：先弹二次确认，再发请求，最后回显结果
  await page.getByRole('spinbutton', { name: '写入值' }).fill('26.5')
  await page.getByRole('button', { name: '写入', exact: true }).click()

  const dialog = page.getByRole('dialog', { name: '写入确认' })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByText('确认向点位「温度」写入值 26.5？')).toBeVisible()
  await dialog.getByRole('button', { name: '确认写入' }).click()

  await expect(page.getByText(/温度 ← 26\.5（结果: ok）/)).toBeVisible()
  expect(captured.body).toEqual({ writes: [{ id: 'p-1', value: 26.5 }] })

  // 写入成功后自动复读，展示设备回读值
  await expect(page.locator('.value-number')).toContainText('26.5')
})
