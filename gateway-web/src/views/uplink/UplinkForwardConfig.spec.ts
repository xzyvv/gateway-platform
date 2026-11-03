import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage, ElSwitch } from 'element-plus'

/**
 * D1：上行 › 通道转发行为测试。
 *
 * 这一页的历史教训很具体：曾经「只要 broker 字段非空就报连接成功」，
 * 现场据此以为数据在发，实际一条都没出去。所以钉住：
 *   1. 「已启用」不等于「已连接」——三态 tag 必须如实反映运行态；
 *   2. 测试结论必须来自真实探测结果，失败原因要翻译成正确的下一步；
 *   3. 空配置 / 全部停用都要给出可点击的下一步，而不是一句「暂无数据」；
 *   4. 开关只改目标通道，不许把另一段配置一起写坏。
 */

const { showToast, push } = vi.hoisted(() => ({ showToast: vi.fn(), push: vi.fn() }))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})

vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({
  useErrorDisplay: () => ({ showToast }),
}))
vi.mock('@/api/config', () => ({ getConfig: vi.fn(), postConfig: vi.fn() }))
vi.mock('@/api/uplink', () => ({
  getUplinkConfig: vi.fn(),
  getNorthStatus: vi.fn(),
  testUplink: vi.fn(),
}))
vi.mock('@/api/webhook', () => ({ testWebhook: vi.fn() }))

import UplinkForwardConfig from './UplinkForwardConfig.vue'
import { getConfig, postConfig } from '@/api/config'
import { getUplinkConfig, getNorthStatus, testUplink } from '@/api/uplink'
import { testWebhook } from '@/api/webhook'
import DataTable from '@/components/DataTable.vue'

interface CfgLike {
  mqtt?: { enabled: boolean; broker: string; topic: string; qos: number; publish_interval: number; username?: string }
  webhook?: { enabled: boolean; url: string; headers?: Record<string, string | undefined> }
  buses?: unknown[]
}

function cfgBoth(): CfgLike {
  return {
    buses: [],
    mqtt: { enabled: true, broker: 'tcp://broker:1883', topic: 'gw/data', qos: 1, publish_interval: 5, username: 'u' },
    webhook: { enabled: false, url: 'https://hook/api', headers: { 'X-Api-Key': undefined } },
  }
}

async function settle() {
  await flushPromises()
  await nextTick()
}

const wrappers: Array<ReturnType<typeof mount>> = []

async function mountPage() {
  const wrapper = mount(UplinkForwardConfig, { attachTo: document.body })
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

async function clickButton(wrapper: ReturnType<typeof mount>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().includes(text))
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  await btn!.trigger('click')
  await settle()
}

/** el-dialog 走 teleport，弹窗内容只能从 document.body 读 */
function dialogText(): string {
  return document.body.textContent ?? ''
}

describe('UplinkForwardConfig 通道转发', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(getConfig).mockResolvedValue(structuredClone(cfgBoth()) as never)
    vi.mocked(postConfig).mockResolvedValue(undefined as never)
    vi.mocked(getNorthStatus).mockResolvedValue({
      healthy: true,
      channels: [
        { name: 'mqtt', type: 'mqtt', enabled: true, connected: true, extra: {} },
        { name: 'webhook', type: 'http', enabled: false, connected: false, extra: {} },
      ],
      servers: [],
      aggregated: { pending_messages: 0 },
    } as never)
    vi.mocked(getUplinkConfig).mockResolvedValue({ template: '{"point_id":"${point_id}"}' } as never)
    vi.mocked(testUplink).mockResolvedValue({ ok: true, publish: { delivered: true, latency_ms: 12, topic: 'gw/test' } } as never)
    vi.mocked(testWebhook).mockResolvedValue({ ok: true, status: 200, status_text: 'OK', latency: 20 } as never)
  })

  afterEach(() => {
    for (const w of wrappers) w.unmount()
    wrappers.length = 0
    document.body.innerHTML = ''
  })

  it('按配置生成通道行：MQTT 与 HTTP 各一行，目标与上报周期正确', async () => {
    const wrapper = await mountPage()
    const table = wrapper.findComponent(DataTable)
    const rows = (table.props() as unknown as { data: Array<{ id: string }> }).data
    expect(rows.map((r) => r.id)).toEqual(['mqtt', 'http'])
    const text = wrapper.text()
    expect(text).toContain('tcp://broker:1883 / gw/data')
    expect(text).toContain('5s')
    expect(text).toContain('https://hook/api')
  })

  it('状态列区分「已停用 / 已启用·已连接 / 已启用·状态未知」三种真相', async () => {
    const wrapper = await mountPage()
    const text = wrapper.text()
    expect(text).toContain('已启用·已连接')
    expect(text).toContain('已停用')
  })

  it('已启用但没连上时显示「已启用·未连接」，不显示成已连接', async () => {
    vi.mocked(getNorthStatus).mockResolvedValue({
      healthy: false,
      channels: [{ name: 'mqtt', type: 'mqtt', enabled: true, connected: false, extra: {} }],
      servers: [],
      aggregated: { pending_messages: 0 },
    } as never)
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('已启用·未连接')
    expect(wrapper.text()).not.toContain('已启用·已连接')
  })

  it('运行态接口不可用时退化为「状态未知」，不谎报已连接', async () => {
    vi.mocked(getNorthStatus).mockRejectedValue(new Error('north down'))
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('已启用·状态未知')
    expect(wrapper.text()).not.toContain('已启用·已连接')
  })

  it('切换启用开关只改目标通道，另一段配置保持原样', async () => {
    const wrapper = await mountPage()
    const switches = wrapper.findAllComponents(ElSwitch)
    expect(switches).toHaveLength(2)

    switches[0]!.vm.$emit('update:modelValue', false)
    switches[0]!.vm.$emit('change', false)
    await settle()

    const body = vi.mocked(postConfig).mock.calls[0]![0] as unknown as CfgLike
    expect(body.mqtt?.enabled).toBe(false)
    expect(body.webhook).toMatchObject({ enabled: false, url: 'https://hook/api' })
    expect(body.webhook?.headers).toEqual({ 'X-Api-Key': undefined })
    expect(ElMessage.success).toHaveBeenCalledWith('已停用')
  })

  it('切换失败时走统一错误提示，不静默吞掉', async () => {
    vi.mocked(postConfig).mockRejectedValue(new Error('disk full'))
    const wrapper = await mountPage()
    const switches = wrapper.findAllComponents(ElSwitch)
    switches[0]!.vm.$emit('update:modelValue', false)
    switches[0]!.vm.$emit('change', false)
    await settle()

    expect(showToast).toHaveBeenCalled()
    expect(showToast.mock.calls[0]![1]).toBe('操作失败')
  })

  it('MQTT 测试成功：显示真实延迟与 Topic，而不是「字段非空即成功」', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '测试')

    expect(testUplink).toHaveBeenCalledWith(
      expect.objectContaining({ send: true, vars: expect.objectContaining({ point_id: 'connection_test' }) }),
    )
    const text = dialogText()
    expect(text).toContain('连接成功')
    expect(text).toContain('延迟：12 ms')
    expect(text).toContain('已向 tcp://broker:1883 成功发布一条测试消息')
    expect(text).toContain('gw/test')
  })

  it('MQTT 发布未送达：判为失败并给出后端原因', async () => {
    vi.mocked(testUplink).mockResolvedValue({ ok: true, publish: { delivered: false, topic: 'gw/test' } } as never)
    const wrapper = await mountPage()
    await clickButton(wrapper, '测试')

    const text = dialogText()
    expect(text).toContain('连接失败')
    expect(text).toContain('消息未成功送达 Broker')
  })

  it('MQTT 客户端未启动 + 通道已停用：提示先打开启用开关', async () => {
    vi.mocked(getConfig).mockResolvedValue(structuredClone({ ...cfgBoth(), mqtt: { ...cfgBoth().mqtt!, enabled: false } }) as never)
    vi.mocked(testUplink).mockResolvedValue({ ok: false, error: 'MQTT 客户端未启动（mqtt.enabled=false）' } as never)
    const wrapper = await mountPage()
    await clickButton(wrapper, '测试')

    const text = dialogText()
    expect(text).toContain('MQTT 通道当前是停用状态')
    expect(text).toContain('启用')
  })

  it('MQTT 客户端未启动 + 通道已启用：指向链路不通，别让用户反复关开开关', async () => {
    vi.mocked(testUplink).mockResolvedValue({ ok: false, error: 'MQTT 客户端未启动（mqtt.enabled=false）' } as never)
    const wrapper = await mountPage()
    await clickButton(wrapper, '测试')

    const text = dialogText()
    expect(text).toContain('MQTT 已启用')
    expect(text).toContain('tcp://broker:1883')
    expect(text).toContain('建立连接')
    expect(text).toContain('网络连通性')
    expect(text).not.toContain('请先在上表打开')
  })

  it('HTTP 测试成功：按真实状态码回显', async () => {
    const wrapper = await mountPage()
    // 第二个「测试」按钮属于 HTTP 行
    const tests = wrapper.findAll('button').filter((b) => b.text() === '测试')
    expect(tests).toHaveLength(2)
    await tests[1]!.trigger('click')
    await settle()

    expect(testWebhook).toHaveBeenCalledWith(
      expect.objectContaining({ url: 'https://hook/api', method: 'POST' }),
    )
    // headers 里缺值统一按空串发出去，不能变成 "undefined"
    const payload = vi.mocked(testWebhook).mock.calls[0]![0] as unknown as { headers: Record<string, string> }
    expect(payload.headers).toEqual({ 'X-Api-Key': '' })

    const text = dialogText()
    expect(text).toContain('连接成功')
    expect(text).toContain('https://hook/api 返回 HTTP 200 OK')
    expect(text).toContain('延迟：20 ms')
  })

  it('HTTP 测试没有延迟数据时不显示「延迟 0 ms」', async () => {
    vi.mocked(testWebhook).mockResolvedValue({ ok: true, status: 204, status_text: 'No Content', latency: null } as never)
    const wrapper = await mountPage()
    const tests = wrapper.findAll('button').filter((b) => b.text() === '测试')
    await tests[1]!.trigger('click')
    await settle()

    const text = dialogText()
    expect(text).toContain('HTTP 204 No Content')
    expect(text).not.toContain('延迟：')
  })

  it('测试请求本身抛异常时弹窗显示原始原因', async () => {
    vi.mocked(testWebhook).mockRejectedValue(new Error('网络不可达'))
    const wrapper = await mountPage()
    const tests = wrapper.findAll('button').filter((b) => b.text() === '测试')
    await tests[1]!.trigger('click')
    await settle()

    const text = dialogText()
    expect(text).toContain('连接失败')
    expect(text).toContain('网络不可达')
  })

  it('配置里没有任何上报段时给出可点击的下一步', async () => {
    vi.mocked(getConfig).mockResolvedValue(structuredClone({ buses: [] }) as never)
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('当前配置里没有 MQTT / HTTP 上报段')
    await clickButton(wrapper, '去配置 MQTT')
    expect(push).toHaveBeenCalledWith({ path: '/system/config', query: { tab: 'runtime-config', section: 'mqtt' } })

    await clickButton(wrapper, '配置 Webhook')
    expect(push).toHaveBeenCalledWith({ path: '/system/config', query: { tab: 'runtime-config', section: 'webhook' } })
  })

  it('通道全部停用时显式提示「网关不会向外上报数据」', async () => {
    vi.mocked(getConfig).mockResolvedValue(
      structuredClone({
        buses: [],
        mqtt: { ...cfgBoth().mqtt!, enabled: false },
        webhook: { ...cfgBoth().webhook!, enabled: false },
      }) as never,
    )
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('所有转发通道当前都处于停用状态')
    await clickButton(wrapper, '前往运行参数')
    // 默认落点优先 MQTT
    expect(push).toHaveBeenCalledWith({ path: '/system/config', query: { tab: 'runtime-config', section: 'mqtt' } })
  })

  it('「参数」按钮按通道类型跳到对应分节（HTTP → webhook）', async () => {
    const wrapper = await mountPage()
    const params = wrapper.findAll('button').filter((b) => b.text() === '参数')
    expect(params).toHaveLength(2)
    await params[1]!.trigger('click')
    await settle()

    expect(push).toHaveBeenCalledWith({ path: '/system/config', query: { tab: 'runtime-config', section: 'webhook' } })
  })
})
