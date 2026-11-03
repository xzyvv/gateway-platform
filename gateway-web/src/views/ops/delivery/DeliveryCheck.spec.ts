import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElCheckbox, ElMessage, ElMessageBox } from 'element-plus'
import DataTable from '@/components/DataTable.vue'

/**
 * D1：交付验收页签行为测试。
 *
 * 交付验收是「现场最后一个动作」，做错了就是把坏网关签收出门，所以钉住：
 *   1. 六项自检的三种结论都要有明确出口（通过 / 有提醒 / 未通过并点名）；
 *   2. 单项接口挂了不能拖累其它项，也不能静默变成「通过」；
 *   3. 检查单的「待确认」必须能一路变成「通过」，且人工备注要真进导出文件；
 *   4. 应用模板是破坏性操作：取消就必须真的什么都没做。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElMessageBox: { confirm: vi.fn() },
  }
})

vi.mock('@/api/status', () => ({ getVersion: vi.fn(), getHealthReport: vi.fn() }))
vi.mock('@/api/config', () => ({ getConfig: vi.fn() }))
vi.mock('@/api/protocols', () => ({ listProtocolCapabilities: vi.fn() }))
vi.mock('@/api/uplink', () => ({ getNorthStatus: vi.fn() }))
vi.mock('@/api/network', () => ({ getNtpStatus: vi.fn() }))
vi.mock('@/api/certificates', () => ({ listCertificates: vi.fn() }))
vi.mock('@/api/watchdog', () => ({ listServices: vi.fn() }))
vi.mock('@/api/project', () => ({ importProject: vi.fn() }))
vi.mock('@/utils/clipboard', () => ({ copyText: vi.fn() }))
vi.mock('@/utils/exportCsv', () => ({ triggerDownload: vi.fn() }))

const { refreshGates } = vi.hoisted(() => ({ refreshGates: vi.fn() }))
vi.mock('@/composables/system/useHealthReport', () => ({
  useHealthReport: () => ({ refresh: refreshGates }),
}))

import DeliveryCheck from './DeliveryCheck.vue'
import { getVersion, getHealthReport } from '@/api/status'
import { getConfig } from '@/api/config'
import { listProtocolCapabilities } from '@/api/protocols'
import { getNorthStatus } from '@/api/uplink'
import { getNtpStatus } from '@/api/network'
import { listCertificates } from '@/api/certificates'
import { listServices } from '@/api/watchdog'
import { importProject } from '@/api/project'
import { copyText } from '@/utils/clipboard'
import { triggerDownload } from '@/utils/exportCsv'

const VERSION = { version: '1.2.3', build_time: '2026-01-01T00:00:00Z', git_hash: 'abc1234' }

function config() {
  return {
    buses: [{ port_name: 'bus-1', protocol: 'modbus' }],
    devices: [{ id: 'dev-1', protocol: 'modbus' }],
    points: [{ id: 'p-1', protocol: 'modbus' }],
  }
}

function health(over: Record<string, unknown> = {}) {
  return {
    summary: { total_devices: 2, online_devices: 2, collecting_devices: 2, bus_count: 1, total_reads: 100, success_rate: 99.5 },
    buses: [
      {
        bus: 'bus-1',
        protocol: 'modbus',
        online_devices: 2,
        total_devices: 2,
        total_reads: 100,
        success_rate: 99.5,
        avg_latency_ms: 10,
        timeout_rate: 0,
        last_error_code: null,
        reconnect_count: 0,
      },
    ],
    ...over,
  }
}

function north() {
  return {
    healthy: true,
    channels: [{ name: 'mqtt-1', type: 'mqtt', enabled: true, connected: true, extra: {} }],
    servers: [],
    aggregated: { pending_messages: 0, total_messages_sent: 10, mqtt: 1, http: 0, mqtt_connected: 1, http_connected: 0 },
  }
}

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(DeliveryCheck)
  await settle()
  return wrapper
}

/** 可点击的容器：VueWrapper 与 DOMWrapper 都满足 */
type Scope = { findAll(selector: string): Array<{ text(): string; trigger(event: string): Promise<void> }> }

/** 点击带指定文案的按钮（Element Plus 的 el-button 最终渲染为原生 button） */
async function clickButton(wrapper: ReturnType<typeof mount>, text: string, scope?: Scope) {
  const buttons = (scope ?? wrapper).findAll('button').filter((b) => b.text() === text)
  expect(buttons[0], `找不到按钮「${text}」`).toBeTruthy()
  await buttons[0]!.trigger('click')
  await settle()
}

/** 读取导出 blob 的文本内容 */
async function blobText(): Promise<string> {
  const call = vi.mocked(triggerDownload).mock.calls[0]!
  return await (call[0] as Blob).text()
}

describe('DeliveryCheck 交付验收', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(getVersion).mockResolvedValue(structuredClone(VERSION) as never)
    vi.mocked(getConfig).mockResolvedValue(structuredClone(config()) as never)
    vi.mocked(listProtocolCapabilities).mockResolvedValue([
      { name: 'modbus' },
      { name: 'iec104' },
    ] as never)
    vi.mocked(getHealthReport).mockResolvedValue(structuredClone(health()) as never)
    vi.mocked(getNorthStatus).mockResolvedValue(structuredClone(north()) as never)
    vi.mocked(getNtpStatus).mockResolvedValue({ status: 'synced', offset_ms: 12, source: 'ntp', last_sync: 1700000000 } as never)
    vi.mocked(listCertificates).mockResolvedValue([{ name: 'gateway', enabled: true, days_remaining: 200 }] as never)
    vi.mocked(listServices).mockResolvedValue([{ name: 'collector', status: 'Running', restarts: 0 }] as never)
    vi.mocked(importProject).mockResolvedValue(undefined as never)
    vi.mocked(copyText).mockResolvedValue(true as never)
    vi.mocked(ElMessageBox.confirm).mockResolvedValue('confirm' as never)
    refreshGates.mockReset()
    globalThis.fetch = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({ buses: [], devices: [], points: [], north: {}, vars: [] }),
    })) as unknown as typeof fetch
  })

  it('未自检时显示空态，检查单全部为「待确认」', async () => {
    const wrapper = await mountPage()
    expect(wrapper.text()).toContain('尚未自检')
    expect(wrapper.findComponent(DataTable).exists()).toBe(false)
    // 六项自动 + 四项人工全部 todo → 总体只能是「待确认」
    expect(wrapper.find('.meta-row').text()).toContain('待确认')
  })

  it('版本接口失败不影响页面可用，导出里版本留空而不是 undefined', async () => {
    vi.mocked(getVersion).mockRejectedValue(new Error('boom'))
    const wrapper = await mountPage()
    expect(wrapper.text()).toContain('Modbus 电力仪表')
    expect(wrapper.text()).toContain('IEC104 变电站')

    await clickButton(wrapper, '导出 JSON')
    const json = JSON.parse(await blobText()) as { gateway: { version: string } }
    expect(json.gateway.version).toBe('')
  })

  it('六项全通过：success 提示、标签与 6 行检查结果', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')

    expect(ElMessage.success).toHaveBeenCalledWith('自检通过，可以交付')
    const table = wrapper.findComponent(DataTable)
    const rows = (table.props() as unknown as { data: unknown[] }).data
    expect(rows).toHaveLength(6)
    expect(wrapper.text()).toContain('自检通过')
  })

  it('自检报告文本包含标题、版本与六项表格行', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')

    const textarea = wrapper.find('textarea').element as HTMLTextAreaElement
    expect(textarea.value).toContain('# 网关交付自检报告')
    expect(textarea.value).toContain('1.2.3')
    expect(textarea.value).toContain('| 检查项 | 结论 | 说明 |')
    expect(textarea.value).toContain('看门狗')
  })

  it('存在提醒项：提示「按说明确认后再交付」，不当成通过', async () => {
    vi.mocked(listCertificates).mockResolvedValue([{ name: 'gateway', enabled: true, days_remaining: 10 }] as never)
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')

    expect(ElMessage.warning).toHaveBeenCalledWith('自检有提醒项，请按说明确认后再交付')
    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('自检提醒')
  })

  it('存在未通过项：提示里点名具体检查项', async () => {
    vi.mocked(getHealthReport).mockResolvedValue(
      structuredClone(
        health({
          buses: [
            {
              bus: 'bus-1',
              protocol: 'modbus',
              online_devices: 0,
              total_devices: 2,
              total_reads: 100,
              success_rate: 0,
              avg_latency_ms: null,
              timeout_rate: 100,
              last_error_code: 'TIMEOUT',
              reconnect_count: 3,
            },
          ],
        }),
      ) as never,
    )
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')

    expect(ElMessage.error).toHaveBeenCalledWith('自检未通过：总线')
    expect(wrapper.text()).toContain('自检未通过')
  })

  it('单项接口失败不中断其它项，失败项写明「接口调用失败」', async () => {
    vi.mocked(getHealthReport).mockRejectedValue(new Error('health 接口 500'))
    vi.mocked(listServices).mockRejectedValue(new Error('watchdog 不可用'))
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')

    const table = wrapper.findComponent(DataTable)
    const rows = (table.props() as unknown as { data: Array<{ id: string; level: string; detail: string }> }).data
    expect(rows).toHaveLength(6)
    const bus = rows.find((r) => r.id === 'bus')!
    const watchdog = rows.find((r) => r.id === 'watchdog')!
    expect(bus.level).toBe('fail')
    expect(bus.detail).toContain('接口调用失败')
    expect(watchdog.detail).toContain('watchdog 不可用')
    // 未被波及的项仍按真实判定给出通过
    expect(rows.find((r) => r.id === 'protocol')!.level).toBe('pass')
  })

  it('勾选人工项后总体结论从「待确认」升到「提醒（可交付）」', async () => {
    const wrapper = await mountPage()
    const boxes = wrapper.findAllComponents(ElCheckbox)
    expect(boxes).toHaveLength(4)
    boxes[0]!.vm.$emit('update:modelValue', true)
    await settle()

    expect(wrapper.find('.meta-row').text()).toContain('提醒（可交付）')
  })

  it('自检通过 + 人工项全勾 + 填写交付人 → 总体「通过」', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')

    const boxes = wrapper.findAllComponents(ElCheckbox)
    for (const box of boxes) box.vm.$emit('update:modelValue', true)
    await wrapper.find('input[placeholder="填写现场工程师姓名"]').setValue('张三')
    await settle()

    // 结论只看标签本身（右侧固定提示语里本来就含「待确认」字样）
    expect(wrapper.find('.meta-row .el-tag').text()).toBe('通过')
  })

  it('导出 JSON 带上交付人、人工备注与正确的 schema 标识', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')
    const boxes = wrapper.findAllComponents(ElCheckbox)
    boxes[0]!.vm.$emit('update:modelValue', true)
    await settle()
    await wrapper.findAll('input[placeholder="备注（可选）"]')[0]!.setValue('已按图核对 RS485 屏蔽')
    await wrapper.find('input[placeholder="填写现场工程师姓名"]').setValue('张三')
    await settle()

    await clickButton(wrapper, '导出 JSON')
    const json = JSON.parse(await blobText()) as {
      kind: string
      operator: string
      gateway: { version: string }
      checks: Array<{ id: string; status: string; note: string; source: string }>
    }
    expect(json.kind).toBe('gateway-delivery-checklist')
    expect(json.operator).toBe('张三')
    expect(json.gateway.version).toBe('1.2.3')
    const wiring = json.checks.find((c) => c.id === 'wiring')!
    expect(wiring.source).toBe('manual')
    expect(wiring.status).toBe('pass')
    expect(wiring.note).toBe('已按图核对 RS485 屏蔽')
    // 自动自检项也必须在同一份检查单里
    expect(json.checks.some((c) => c.source === 'auto' && c.id === 'protocol')).toBe(true)
  })

  it('导出 Markdown 含检查单标题、交付人与备注', async () => {
    const wrapper = await mountPage()
    const boxes = wrapper.findAllComponents(ElCheckbox)
    boxes[0]!.vm.$emit('update:modelValue', true)
    await settle()
    await wrapper.findAll('input[placeholder="备注（可选）"]')[0]!.setValue('屏蔽层已接地')
    await settle()

    await clickButton(wrapper, '导出 Markdown')
    const md = await blobText()
    expect(md).toContain('# 网关交付检查单')
    expect(md).toContain('屏蔽层已接地')
    expect(md).toContain('待确认')
  })

  it('取消应用模板：不下载、不导入、不刷新健康报告', async () => {
    vi.mocked(ElMessageBox.confirm).mockRejectedValue('cancel')
    const wrapper = await mountPage()
    const row = wrapper.findAll('.tpl-row')[0]!
    await clickButton(wrapper, '应用模板', row)

    expect(globalThis.fetch).not.toHaveBeenCalled()
    expect(importProject).not.toHaveBeenCalled()
    expect(refreshGates).not.toHaveBeenCalled()
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('确认应用模板：导入模板内容并强制刷新健康报告', async () => {
    const tpl = { buses: [{ port_name: 'COM3' }], devices: [], points: [], north: {}, vars: [] }
    globalThis.fetch = vi.fn(async () => ({ ok: true, status: 200, json: async () => tpl })) as unknown as typeof fetch
    const wrapper = await mountPage()
    const row = wrapper.findAll('.tpl-row')[0]!
    await clickButton(wrapper, '应用模板', row)

    expect(ElMessageBox.confirm).toHaveBeenCalled()
    expect(importProject).toHaveBeenCalledWith(tpl)
    expect(refreshGates).toHaveBeenCalledWith(true)
    expect(ElMessage.success).toHaveBeenCalledWith('模板已应用，请点击「一键自检」确认采集与链路状态')
  })

  it('模板下载 404：提示致命失败且不触发下载', async () => {
    globalThis.fetch = vi.fn(async () => ({ ok: false, status: 404, json: async () => null })) as unknown as typeof fetch
    const wrapper = await mountPage()
    await clickButton(wrapper, '下载 JSON', wrapper.findAll('.tpl-row')[0]!)

    expect(ElMessage.error).toHaveBeenCalledWith('下载模板失败：HTTP 404')
    expect(triggerDownload).not.toHaveBeenCalled()
  })

  it('下载模板成功：按模板文件名下载', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '下载 JSON', wrapper.findAll('.tpl-row')[0]!)

    expect(triggerDownload).toHaveBeenCalledOnce()
    expect(vi.mocked(triggerDownload).mock.calls[0]![1]).toBe('modbus-power-meter.json')
    expect(ElMessage.success).toHaveBeenCalledWith('模板已下载')
  })

  it('复制报告：把完整 Markdown 交给剪贴板工具', async () => {
    const wrapper = await mountPage()
    await clickButton(wrapper, '一键自检')
    await clickButton(wrapper, '复制报告')

    expect(copyText).toHaveBeenCalledOnce()
    const [text, okMessage] = vi.mocked(copyText).mock.calls[0]!
    expect(text).toContain('# 网关交付自检报告')
    expect(okMessage).toBe('自检报告已复制')
  })
})
