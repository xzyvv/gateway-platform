import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { ElMessage } from 'element-plus'

const m = vi.hoisted(() => ({
  push: vi.fn(),
  testUplink: vi.fn(),
  getUplinkConfig: vi.fn(),
  listTestHistory: vi.fn(),
  clearTestHistory: vi.fn(),
  confirmDelete: vi.fn(),
}))

vi.mock('vue-router', () => ({ useRouter: () => ({ push: m.push }) }))
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})
vi.mock('@/utils/confirm', () => ({ confirmDelete: (...a: unknown[]) => m.confirmDelete(...a) }))
vi.mock('@/api/uplink', () => {
  const BUILTIN_TEMPLATES = [
    { id: 'default', name: '默认', description: '', template: '{"a":${value}}' },
    { id: 'tb', name: 'ThingsBoard', description: '', template: '{"ts":${timestamp_ms}}' },
  ]
  return {
    BUILTIN_TEMPLATES,
    testUplink: (p: unknown) => m.testUplink(p),
    getUplinkConfig: () => m.getUplinkConfig(),
    listTestHistory: (l: number) => m.listTestHistory(l),
    clearTestHistory: () => m.clearTestHistory(),
  }
})

import { useUplinkTester } from './useUplinkTester'

type Cli = ReturnType<typeof useUplinkTester>

const mounted: Array<{ unmount: () => void }> = []

function harness() {
  let api: Cli | null = null
  const Comp = defineComponent({
    setup() {
      api = useUplinkTester()
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  mounted.push(wrapper)
  return { wrapper, get api() { return api! } }
}

describe('useUplinkTester', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    m.getUplinkConfig.mockResolvedValue({ enabled: true, template: '{"from":"cfg"}', encoding: 'json', max_bytes: 100 })
    m.listTestHistory.mockResolvedValue([])
    m.testUplink.mockResolvedValue({ ok: true, rendered: '{}', bytes: 2 })
    m.confirmDelete.mockResolvedValue(true)
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText: vi.fn().mockResolvedValue(undefined) },
      configurable: true,
    })
  })

  afterEach(() => {
    mounted.forEach((w) => w.unmount())
    mounted.length = 0
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  it('挂载后加载历史并填充随机示例数据', async () => {
    const { api } = harness()
    await flushPromises()
    expect(m.listTestHistory).toHaveBeenCalledWith(50)
    expect(api.history.value).toEqual([])
    expect(['电表1', 'PLC_03', '水表_001', '温湿度_01']).toContain(api.state.vars.device_id)
  })

  it('rendered 前端兜底渲染：整数/小数/布尔/缺失变量语义', () => {
    const { api } = harness()
    api.state.template = '${value}'
    api.state.vars.value = 220
    expect(api.rendered.value).toBe('220')
    api.state.vars.value = 220.5
    expect(api.rendered.value).toBe('220.5')
    api.state.vars.value = NaN
    expect(api.rendered.value).toBe('null')

    api.state.template = '${flag}'
    api.state.vars.flag = true
    expect(api.rendered.value).toBe('true')

    api.state.template = 'x${missing}y'
    expect(api.rendered.value).toBe('xy')
  })

  it('valueNum / staleNum 双向 computed 约束取值', () => {
    const { api } = harness()
    api.valueNum.value = 3.7
    expect(api.state.vars.value).toBe(3.7)
    expect(api.valueNum.value).toBe(3.7)
    api.valueNum.value = Infinity
    expect(api.state.vars.value).toBe(0)

    api.staleNum.value = 3.9
    expect(api.state.vars.stale_count).toBe(3)
    api.staleNum.value = -5
    expect(api.state.vars.stale_count).toBe(0)
  })

  it('compileError 与字节数 computed', () => {
    const { api } = harness()
    api.state.template = '{"a": ${value'
    expect(api.compileError.value).toContain('未闭合')
    api.state.template = '{"a": ${value}}'
    expect(api.compileError.value).toBeNull()
    expect(api.templateBytes.value).toBe(new Blob(['{"a": ${value}}']).size)
    expect(api.renderedBytes.value).toBe(new Blob([api.rendered.value]).size)
  })

  it('applyTemplate 命中/未命中内置模板', () => {
    const { api } = harness()
    api.applyTemplate('tb')
    expect(api.state.template).toBe('{"ts":${timestamp_ms}}')
    expect(ElMessage.success).toHaveBeenCalledWith('已应用：ThingsBoard')
    const before = api.state.template
    api.applyTemplate('none')
    expect(api.state.template).toBe(before)
  })

  it('randomizeData 用随机样本覆盖变量并提示', () => {
    const { api } = harness()
    const spy = vi.spyOn(Math, 'random').mockReturnValue(0)
    api.randomizeData()
    expect(api.state.vars.device_id).toBe('电表1')
    expect(api.state.vars.unit).toBe('V')
    expect(ElMessage.success).toHaveBeenCalledWith('已填入随机示例数据')
    spy.mockRestore()
  })

  it('loadFromConfig 成功覆盖模板，失败保留并警告', async () => {
    const { api } = harness()
    await flushPromises()
    await api.loadFromConfig()
    expect(api.state.template).toBe('{"from":"cfg"}')
    expect(ElMessage.success).toHaveBeenCalledWith('已加载保存的模板')

    m.getUplinkConfig.mockRejectedValue(new Error('404'))
    api.state.template = 'keep'
    await api.loadFromConfig()
    expect(api.state.template).toBe('keep')
    expect(ElMessage.warning).toHaveBeenCalledWith('后端未提供 /api/uplink/config，使用本地默认')
  })

  it('copyRendered 成功/失败分别提示', async () => {
    const { api } = harness()
    await api.copyRendered()
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith(api.rendered.value)
    expect(ElMessage.success).toHaveBeenCalledWith('已复制到剪贴板')

    vi.mocked(navigator.clipboard.writeText).mockRejectedValueOnce(new Error('denied'))
    await api.copyRendered()
    expect(ElMessage.warning).toHaveBeenCalledWith('剪贴板访问失败，请手动复制')
  })

  it('reuseHistory 复用模板与变量；变量 JSON 非法时仅复用模板', () => {
    const { api } = harness()
    api.reuseHistory({ id: 1, timestamp: 0, template: 'T', rendered: '', vars_json: '{"value":9}', send: false, delivered: false, latency_ms: 0 })
    expect(api.state.template).toBe('T')
    expect(api.state.vars.value).toBe(9)
    expect(ElMessage.success).toHaveBeenCalledWith('已复用历史记录的模板与变量')

    api.reuseHistory({ id: 2, timestamp: 0, template: 'T2', rendered: '', vars_json: '{bad', send: false, delivered: false, latency_ms: 0 })
    expect(api.state.template).toBe('T2')
    expect(ElMessage.warning).toHaveBeenCalledWith('历史变量解析失败，仅复用模板')
  })

  it('testSend：真实发送成功走 success 并刷新历史；未送达走 warning；失败走 error', async () => {
    const { api } = harness()
    await flushPromises()
    m.listTestHistory.mockClear()

    m.testUplink.mockResolvedValue({ ok: true, rendered: 'r', bytes: 1, publish: { topic: 't/q', qos: 1, delivered: true, latency_ms: 3 } })
    await api.testSend(true)
    expect(m.testUplink).toHaveBeenCalledWith(expect.objectContaining({ send: true }))
    expect(ElMessage.success).toHaveBeenCalledWith('已送达 t/q')
    expect(m.listTestHistory).toHaveBeenCalled()
    expect(api.lastResult.value?.ok).toBe(true)
    expect(api.sending.value).toBe(false)

    m.testUplink.mockResolvedValue({ ok: true, rendered: 'r', bytes: 1, publish: { topic: 't/q', qos: 1, delivered: false, latency_ms: 3 } })
    await api.testSend(true)
    expect(ElMessage.warning).toHaveBeenCalledWith('渲染成功但未送达')

    m.testUplink.mockResolvedValue({ ok: false, rendered: '', bytes: 0, error: 'bad' })
    await api.testSend(false)
    expect(ElMessage.error).toHaveBeenCalledWith('bad')
  })

  it('confirmClearHistory：确认后清空历史，取消则不动作', async () => {
    const { api } = harness()
    await flushPromises()
    api.history.value = [{ id: 1, timestamp: 0, template: 'T', rendered: '', vars_json: '{}', send: true, delivered: true, latency_ms: 1 }]
    m.confirmDelete.mockResolvedValue(true)
    m.clearTestHistory.mockResolvedValue({ cleared: true })
    await api.confirmClearHistory()
    expect(m.clearTestHistory).toHaveBeenCalledTimes(1)
    expect(api.history.value).toEqual([])
    expect(ElMessage.success).toHaveBeenCalledWith('测试历史已清空')

    m.confirmDelete.mockResolvedValue(false)
    await api.confirmClearHistory()
    expect(m.clearTestHistory).toHaveBeenCalledTimes(1)
  })

  it('goConfig 跳转，formatTime 补零', () => {
    const { api } = harness()
    api.goConfig()
    expect(m.push).toHaveBeenCalledWith('/uplink?tab=forward-config')
    const t = new Date(2024, 0, 2, 3, 4, 5).getTime()
    expect(api.formatTime(t)).toBe('03:04:05')
  })
})