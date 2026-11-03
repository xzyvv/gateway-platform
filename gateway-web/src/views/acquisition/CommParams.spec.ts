import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElInputNumber, ElMessage, ElNotification, ElSelect } from 'element-plus'

/**
 * D1：采集调优 › 通信参数（总线级通信参数）行为测试。
 *
 * 这里守的是「现场调参」最容易出事的三条链路：
 *   1. 参数改完是否真的写回**完整配置**（写半份配置 = 现场参数被静默重置）；
 *   2. 写失败时用户是否得到明确提示（静默失败 = 现场以为改了其实没改）；
 *   3. 字节序是设备级字段，批量接口失败必须说清「结果未知」而不是让用户猜。
 */

const { showToast } = vi.hoisted(() => ({ showToast: vi.fn() }))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElNotification: vi.fn(),
  }
})

vi.mock('@/api/config', () => ({
  getConfig: vi.fn(),
  postConfig: vi.fn(),
}))

vi.mock('@/api/points', () => ({
  batchEditPoints: vi.fn(),
}))

vi.mock('@/composables/core/useErrorDisplay', () => ({
  useErrorDisplay: () => ({ showToast }),
}))

import CommParams from './CommParams.vue'
import { getConfig, postConfig } from '@/api/config'
import { batchEditPoints } from '@/api/points'

interface Cfg {
  buses: Array<Record<string, unknown>>
  devices: Array<Record<string, unknown>>
  points: Array<Record<string, unknown>>
}

function fixture(): Cfg {
  return {
    buses: [
      { port_name: 'bus-1', protocol: 'modbus', timeout_ms: 1000, retry_count: 2, retry_delay_ms: 500 },
      { port_name: 'bus-2', protocol: 'modbus', timeout_ms: 2000, retry_count: 1, retry_delay_ms: 300 },
    ],
    devices: [
      { id: 'dev-1', name: '电表一号', bus: 'bus-1' },
      { id: 'dev-2', name: '电表二号', bus: 'bus-1' },
      { id: 'dev-3', name: '压力表', bus: 'bus-2' },
    ],
    points: [{ device_id: 'dev-1', byte_order: 'ABCD' }],
  }
}

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(CommParams)
  await settle()
  return wrapper
}

/** 触发某个 ElInputNumber 的取值变更 + change（页面在 change 时才发起保存） */
async function setNumber(wrapper: ReturnType<typeof mount>, index: number, value: number) {
  const input = wrapper.findAllComponents(ElInputNumber)[index]!
  expect(input).toBeTruthy()
  input.vm.$emit('update:modelValue', value)
  // ElInputNumber 的 change 事件带值（Element Plus 会对事件参数做校验）
  input.vm.$emit('change', value)
  await settle()
}

/** 触发某个 ElSelect 的取值变更 + change */
async function setSelect(wrapper: ReturnType<typeof mount>, index: number, value: string) {
  const select = wrapper.findAllComponents(ElSelect)[index]!
  expect(select).toBeTruthy()
  select.vm.$emit('update:modelValue', value)
  select.vm.$emit('change', value)
  await settle()
}

describe('CommParams 总线级通信参数', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(getConfig).mockImplementation(async () => structuredClone(fixture()) as never)
    vi.mocked(postConfig).mockResolvedValue(undefined as never)
    vi.mocked(batchEditPoints).mockResolvedValue(undefined as never)
  })

  it('按总线成行展示，设备数按所属总线统计', async () => {
    const wrapper = await mountPage()
    const text = wrapper.text()
    expect(text).toContain('bus-1')
    expect(text).toContain('bus-2')
    // bus-1 挂 2 台、bus-2 挂 1 台：行内设备数不能按设备行算
    expect(text).toContain('2 台')
    expect(text).toContain('1 台')
  })

  it('改超时后提交完整配置：只改目标总线，其余总线保持原值', async () => {
    const wrapper = await mountPage()
    await setNumber(wrapper, 0, 1500)

    expect(postConfig).toHaveBeenCalledOnce()
    const body = vi.mocked(postConfig).mock.calls[0]![0] as unknown as Cfg
    expect(body.buses[0]).toMatchObject({ port_name: 'bus-1', timeout_ms: 1500 })
    // 未编辑的字段与其它总线不得被顺带改掉
    expect(body.buses[0]).toMatchObject({ retry_count: 2, retry_delay_ms: 500 })
    expect(body.buses[1]).toMatchObject({ port_name: 'bus-2', timeout_ms: 2000 })
  })

  it('保存成功给出「影响 N 台设备」的口径提示', async () => {
    const wrapper = await mountPage()
    await setNumber(wrapper, 0, 1500)

    expect(ElMessage.success).toHaveBeenCalledWith('已保存总线 bus-1 的参数（影响 2 台设备）')
  })

  it('保存失败：toast 明确提示，不静默吞掉', async () => {
    vi.mocked(postConfig).mockRejectedValue(new Error('disk full'))
    const wrapper = await mountPage()
    await setNumber(wrapper, 0, 1500)

    expect(showToast).toHaveBeenCalled()
    const [err, fallback] = showToast.mock.calls[0]!
    expect(fallback).toBe('保存失败')
    expect((err as Error).message).toBe('disk full')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('总线已被删除：提示刷新重试，且不提交任何配置', async () => {
    vi.mocked(getConfig)
      .mockResolvedValueOnce(structuredClone(fixture()) as never)
      .mockResolvedValueOnce({ buses: [], devices: [], points: [] } as never)

    const wrapper = await mountPage()
    await setNumber(wrapper, 0, 1500)

    expect(ElMessage.warning).toHaveBeenCalledWith('总线 bus-1 已不存在，请刷新后重试')
    expect(postConfig).not.toHaveBeenCalled()
  })
})

describe('CommParams 设备级字节序', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(getConfig).mockImplementation(async () => structuredClone(fixture()) as never)
    vi.mocked(postConfig).mockResolvedValue(undefined as never)
    vi.mocked(batchEditPoints).mockResolvedValue(undefined as never)
  })

  it('改字节序：该设备的全部点位一起更新并提交', async () => {
    const wrapper = await mountPage()
    await setSelect(wrapper, 0, 'DCBA')

    expect(batchEditPoints).toHaveBeenCalledOnce()
    const payload = vi.mocked(batchEditPoints).mock.calls[0]![0] as unknown as {
      update: Array<{ device_id: string; byte_order: string }>
    }
    expect(payload.update.length).toBe(1)
    expect(payload.update[0]).toMatchObject({ device_id: 'dev-1', byte_order: 'DCBA' })
    expect(ElMessage.success).toHaveBeenCalledWith('已更新 电表一号 的字节序')
  })

  it('设备无点位：提示「该设备无点位」且不调用批量接口', async () => {
    const wrapper = await mountPage()
    // dev-3（压力表）没有点位：最后一个字节序下拉
    const selectCount = wrapper.findAllComponents(ElSelect).length
    await setSelect(wrapper, selectCount - 1, 'CDAB')

    expect(ElMessage.warning).toHaveBeenCalledWith('该设备无点位')
    expect(batchEditPoints).not.toHaveBeenCalled()
  })

  it('字节序保存失败：额外提示「结果未知」，不假装全部失败', async () => {
    vi.mocked(batchEditPoints).mockRejectedValue(new Error('timeout'))
    const wrapper = await mountPage()
    await setSelect(wrapper, 0, 'DCBA')

    expect(showToast).toHaveBeenCalled()
    expect(ElNotification).toHaveBeenCalled()
    const arg = vi.mocked(ElNotification).mock.calls[0]![0] as { title?: string; message?: string }
    expect(arg.title).toBe('字节序保存结果未知')
    expect(arg.message).toContain('逐条核对')
  })
})
