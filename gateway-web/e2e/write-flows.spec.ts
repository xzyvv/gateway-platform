import { test, expect, type Route } from '@playwright/test'
import { loginViaForm, stubLanDiscovery } from './helpers'

/**
 * 写链路 E2E：配置保存、点位新增/删除。
 *
 * 两条链路都必须经过「校验 → 确认 → 写请求 → 成功提示」，任何一环退化成
 * 静默或空提示，用户都会以为没生效。写接口全部用 page.route 拦截并返回成功，
 * 不打到网关（E2E 网关是空配置只读环境，真实写入会污染后续用例）。
 */

async function fulfillJson(route: Route, body: unknown, status = 200): Promise<void> {
  await route.fulfill({ status, contentType: 'application/json', body: JSON.stringify(body) })
}

test.beforeEach(async ({ page }) => {
  await stubLanDiscovery(page)
})

test('配置保存：校验通过 → 确认 → POST /api/config → 成功提示', async ({ page }) => {
  const captured: { body?: Record<string, unknown> } = {}

  // 校验规则在后端，直接返回"无问题"
  await page.route('**/api/config/validate', (route) => fulfillJson(route, []))
  await page.route('**/api/config', async (route) => {
    if (route.request().method() !== 'POST') return route.continue()
    captured.body = route.request().postDataJSON() as Record<string, unknown>
    await fulfillJson(route, { restart_required: [] })
  })

  await loginViaForm(page)
  await page.goto('/system/config?tab=runtime-config')

  // P0-4 命名统一后主按钮为「保存并应用」（热重载是保存的副作用，不再出现在按钮名里）
  const save = page.getByRole('button', { name: /保存并应用/ })
  await expect(save).toBeEnabled()
  await save.click()

  // 保存前必须有一次显式确认（防误触发热重载）
  await page.getByRole('button', { name: '确认保存' }).click()

  await expect(page.getByText('配置已保存并热重载')).toBeVisible()
  expect(captured.body).toBeTruthy()
  // 请求体必须是完整配置对象，而不是空壳
  expect(captured.body).toHaveProperty('buses')
})

test('点位新增与删除：POST / DELETE 各走通一次并给出成功提示', async ({ page }) => {
  // 与 DeviceConfig 生成类型对齐（ts-rs）：缺 interval 会让 prefillFromDevice 取到 undefined，
  // 缺 template/address/slave_id 等则字段语义不完整，表单预填与提交都不可信。
  const device = {
    id: 'dev1',
    name: '设备1',
    template: 'modbus-holding',
    bus: 'bus1',
    protocol: 'modbus',
    address: '1',
    slave_id: 1,
    address_map: null,
    interval: 5,
  }
  const existing = {
    id: 'p1',
    name: '温度',
    device_id: 'dev1',
    bus: 'bus1',
    protocol: 'modbus',
    address: '40001',
    slave_id: 1,
    interval: 60,
    data_type: 'u16',
    writable: false,
  }
  const created: { body?: Record<string, unknown> } = {}
  let deleted = false

  await page.route('**/api/devices', (route) => fulfillJson(route, [device]))
  await page.route('**/api/devices/dev1/points', async (route) => {
    if (route.request().method() === 'POST') {
      created.body = route.request().postDataJSON() as Record<string, unknown>
      return fulfillJson(route, { ...existing, id: String(created.body?.id ?? 'p2') })
    }
    await fulfillJson(route, [existing])
  })
  await page.route('**/api/points/p1', async (route) => {
    if (route.request().method() === 'DELETE') {
      deleted = true
      return fulfillJson(route, { status: 'ok' })
    }
    await route.continue()
  })

  await loginViaForm(page)
  // 用深链预置 tab + 设备：PointList 的 watch(devices, immediate) 会从 ?device= 还原选中设备，
  // 无需再点一次下拉，让本用例专注「新增 → 删除」这条写链路。
  await page.goto('/points?tab=points&device=dev1')

  // 设备已由深链选中，直接等表格首行
  await expect(page.locator('.el-select__wrapper').first()).toContainText('设备1')
  await expect(page.locator('.el-table__row').first()).toBeVisible()

  await page.getByRole('button', { name: /新增点位/ }).click()
  const dialog = page.getByRole('dialog', { name: '新增点位' })
  await dialog.getByPlaceholder('如: temp_01').fill('p2')
  await dialog.getByPlaceholder('如: 温度传感器').fill('压力')
  // 总线/协议已改为从所属设备继承（禁用输入框，placeholder「继承自所属设备」），
  // 不再允许手填，这里不再填 bus
  await expect(dialog.getByPlaceholder('继承自所属设备').first()).toHaveValue('bus1')
  // 地址 placeholder 由后端协议描述符下发，未加载时会回落「设备地址」，
  // 不能硬编码 Modbus 文案；按表单项 label 定位更稳。
  await dialog.getByLabel('地址', { exact: true }).fill('40002')
  // 提交前复检：openCreate 与 submitPoint 都以 selectedDevice 为前置，
  // 若它在打开对话框后被清空，submitPoint 会静默 return（无请求、无校验错误）。
  await expect(page.locator('.el-select__wrapper').first()).toContainText('设备1')
  // 回归点：pointFormRef 曾被漏在 PointList 的解构列表之外，
  // 导致 validate() 拿到 undefined 而静默 return —— 点「创建」永远无反应。
  await dialog.getByRole('button', { name: '创建' }).click()
  await expect(page.getByText('点位创建成功')).toBeVisible()
  expect(created.body).toMatchObject({ id: 'p2', address: '40002', interval: expect.any(Number) })

  // 删除走行内「更多」下拉 + 确认框
  const row = page.locator('.el-table__row').first()
  await row.getByRole('button', { name: '更多' }).click()
  await page.getByRole('menuitem', { name: '删除' }).click()
  await page.getByRole('dialog').getByRole('button', { name: '删除' }).click()

  await expect(page.getByText('删除成功')).toBeVisible()
  expect(deleted).toBe(true)
})

/**
 * P2-3：真实写链路。前面的用例全部 page.route mock，写请求从未真正到达网关，
 * 后端校验/落盘/落版本任何一环坏掉都测不出来。这里对**隔离的 E2E 网关**
 * （独立端口 18080 + 独立临时目录，见 scripts/e2e/start-gateway.mjs）做一次
 * 真实保存，串起三件事：
 *   1. P2-2：GET /api/config 返回 ApiResponse 信封（code/message/data）
 *   2. P2-1：启动即落基线快照，版本列表非空（不再出现"版本页永远为空"）
 *   3. 写链路真实生效：POST /api/config 成功，且版本历史只追加一条
 *
 * 注意：该用例不 mock 任何请求；端口隔离保证不会打到 8080 上的现场网关。
 */
test('真实写链路：保存配置落版本快照（隔离网关，不 mock）', async ({ request }) => {
  const login = await request.post('/api/auth/login', {
    data: { username: 'admin', password: 'admin' },
  })
  expect(login.ok()).toBeTruthy()
  const { token } = (await login.json()) as { token: string }
  const headers = { Authorization: `Bearer ${token}` }

  // 1) 信封统一：GET /api/config 不再返回裸对象
  const cfgResp = await request.get('/api/config', { headers })
  expect(cfgResp.status()).toBe(200)
  const envelope = (await cfgResp.json()) as {
    code: number
    message: string
    data: Record<string, unknown>
  }
  expect(envelope.code).toBe(0)
  expect(envelope.data).toBeTruthy()
  expect(envelope.data).toHaveProperty('buses')

  // 2) 基线快照：空配置网关启动后版本库至少有一条（P2-1）
  const before = await request.get('/api/config/versions', { headers })
  expect(before.ok()).toBeTruthy()
  // /api/config/versions 同样走 ApiResponse 信封（code/data/message），需解包 .data
  const beforeBody = (await before.json()) as { code: number; data: { versions: unknown[] } }
  expect(beforeBody.code).toBe(0)
  expect(beforeBody.data.versions.length).toBeGreaterThanOrEqual(1)

  // 3) 真实保存：原样提交当前配置（校验 → 热重载判定 → 落盘 → 落版本）
  const saveResp = await request.post('/api/config', {
    headers,
    data: envelope.data,
  })
  expect(saveResp.ok()).toBeTruthy()
  const saved = (await saveResp.json()) as { code: number; data: { status: string } }
  expect(saved.code).toBe(0)
  expect(saved.data.status).toBe('ok')

  // 版本历史只追加：成功保存必须新增一条快照
  const after = await request.get('/api/config/versions', { headers })
  const afterBody = (await after.json()) as { code: number; data: { versions: unknown[] } }
  expect(afterBody.data.versions.length).toBe(beforeBody.data.versions.length + 1)
})

test('选设备后 URL 自动补回 ?tab=：页签定位不丢，内容区不卸载', async ({ page }) => {
  // 两台设备：进页时 composable 自动选中第一台，手动切到第二台才会触发 onDeviceChange
  const mkDevice = (id: string, name: string) => ({
    id, name, template: 'modbus-holding', bus: 'bus1', protocol: 'modbus',
    address: '1', slave_id: 1, address_map: null, interval: 5,
  })
  const basePoint = {
    name: '温度', bus: 'bus1', protocol: 'modbus', address: '40001',
    slave_id: 1, interval: 60, data_type: 'u16', writable: false,
  }
  await page.route('**/api/devices', (route) =>
    fulfillJson(route, [mkDevice('dev1', '设备1'), mkDevice('dev2', '设备2')]),
  )
  await page.route('**/api/devices/dev1/points', (route) =>
    fulfillJson(route, [{ ...basePoint, id: 'p1', device_id: 'dev1' }]),
  )
  await page.route('**/api/devices/dev2/points', (route) =>
    fulfillJson(route, [{ ...basePoint, id: 'p2', device_id: 'dev2', address: '40002' }]),
  )

  await loginViaForm(page)
  // 故意不带 ?tab=，复现「从侧栏进入 Hub」的默认入口
  await page.goto('/points')

  // 未带 ?device= 时不会自动选中设备，先手动选第二台才会触发 onDeviceChange
  await page.locator('.el-select__wrapper').first().click()
  await page.locator('.el-select-dropdown__item').filter({ hasText: '设备2' }).click()

  // 回归点：useQueryState 重写 query 只保留已有 key，此前 URL 会停在 ?device=dev2（丢 tab），
  // 刷新/分享后页签定位丢失。修复后必须两个 key 都在，且内容区没有被卸载。
  await expect(page).toHaveURL(/tab=points/)
  await expect(page).toHaveURL(/device=dev2/)
  await expect(page.locator('.el-table__row').first()).toBeVisible()
})
