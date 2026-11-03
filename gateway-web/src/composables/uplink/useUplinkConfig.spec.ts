import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { ElMessage, ElNotification } from 'element-plus'

const m = vi.hoisted(() => ({
  push: vi.fn(),
  getUplinkConfig: vi.fn(),
  saveUplinkConfig: vi.fn(),
  listTemplateVars: vi.fn(),
  exportTemplate: vi.fn(),
  importTemplate: vi.fn(),
  logWarn: vi.fn(),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: m.push }),
  useRoute: () => ({ path: '/test-uplink' }),
}))
vi.mock('@/utils/dirtyGuard', () => ({
  registerDirtyGuard: () => ({ reset: vi.fn(), mark: vi.fn(), unregister: vi.fn() }),
}))
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElNotification: Object.assign(vi.fn(), {
      success: vi.fn(),
      error: vi.fn(),
      warning: vi.fn(),
      info: vi.fn(),
    }),
  }
})
vi.mock('@/api/uplink', () => {
  const BUILTIN_TEMPLATES = [
    { id: 'default', name: '默认', description: '', template: '{"a":${value}}' },
    { id: 'tb', name: 'ThingsBoard', description: '', template: '{"ts":${timestamp_ms}}' },
  ]
  return {
    BUILTIN_TEMPLATES,
    getUplinkConfig: () => m.getUplinkConfig(),
    saveUplinkConfig: (c: unknown) => m.saveUplinkConfig(c),
    listTemplateVars: () => m.listTemplateVars(),
    exportTemplate: () => m.exportTemplate(),
    importTemplate: (p: unknown) => m.importTemplate(p),
  }
})
vi.mock('@/utils/logger', () => ({ logWarn: (...a: unknown[]) => m.logWarn(...a) }))

import { useUplinkConfig } from './useUplinkConfig'

const mounted: Array<{ unmount: () => void }> = []

function harness() {
  let api: ReturnType<typeof useUplinkConfig> | null = null
  const Comp = defineComponent({
    setup() {
      api = useUplinkConfig()
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  mounted.push(wrapper)
  return { wrapper, get api() { return api! } }
}

describe('useUplinkConfig', () => {
  let clickSpy: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.clearAllMocks()
    clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
    m.getUplinkConfig.mockResolvedValue({ enabled: false, template: '{"x":${value}}', encoding: 'json', max_bytes: 4096 })
    m.listTemplateVars.mockResolvedValue([{ name: 'value', type: 'number', description: 'v' }])
    m.saveUplinkConfig.mockImplementation(async (c: unknown) => c)
    m.exportTemplate.mockResolvedValue({ template: 't', enabled: true, encoding: 'json', max_bytes: 100, exported_at: 'now' })
    m.importTemplate.mockResolvedValue({})
  })

  afterEach(() => {
    mounted.forEach((w) => w.unmount())
    mounted.length = 0
    clickSpy.mockRestore()
  })

  it('挂载时加载配置与变量清单', async () => {
    const { api } = harness()
    await flushPromises()
    expect(m.getUplinkConfig).toHaveBeenCalledTimes(1)
    expect(api.cfg.value.template).toBe('{"x":${value}}')
    expect(api.loading.value).toBe(false)
    expect(api.templateVars.value).toHaveLength(1)
    expect(api.totalVariables.value).toBe(1)
  })

  it('listTemplateVars 失败时保持空清单并记 warning（不再用前端硬编码兜底）', async () => {
    m.listTemplateVars.mockRejectedValue(new Error('404'))
    const { api } = harness()
    await flushPromises()
    expect(api.templateVars.value).toHaveLength(0)
    expect(m.logWarn).toHaveBeenCalled()
  })

  it('compileError：未闭合 ${ 报错，闭合/空模板为 null', () => {
    const { api } = harness()
    api.cfg.value.template = '{"a": ${value'
    expect(api.compileError.value).toContain('未闭合')
    api.cfg.value.template = '{"a": ${value}}'
    expect(api.compileError.value).toBeNull()
    api.cfg.value.template = ''
    expect(api.compileError.value).toBeNull()
  })

  it('templateBytes/variableCount 随模板变化', () => {
    const { api } = harness()
    api.cfg.value.template = 'a${x} b${y}'
    expect(api.variableCount.value).toBe(2)
    expect(api.templateBytes.value).toBe(new Blob(['a${x} b${y}']).size)
  })

  it('applyTemplate 命中内置模板则更新并提示，未命中不变', () => {
    const { api } = harness()
    api.applyTemplate('tb')
    expect(api.cfg.value.template).toBe('{"ts":${timestamp_ms}}')
    expect(ElMessage.success).toHaveBeenCalledWith('已应用模板：ThingsBoard')

    const before = api.cfg.value.template
    api.applyTemplate('nope')
    expect(api.cfg.value.template).toBe(before)
  })

  it('insertVar 追加变量，行尾已有换行时不重复插入', () => {
    const { api } = harness()
    api.cfg.value.template = 'abc'
    api.insertVar('value')
    expect(api.cfg.value.template).toBe('abc\n${value}')

    api.cfg.value.template = 'abc\n'
    api.insertVar('x')
    expect(api.cfg.value.template).toBe('abc\n${x}')
    expect(ElMessage.success).toHaveBeenCalledWith('已追加 ${x}')
  })

  it('loadConfig 失败时记录 warn 且保留默认配置', async () => {
    const { api } = harness()
    await flushPromises()
    const defaultTpl = api.cfg.value.template
    m.getUplinkConfig.mockRejectedValue(new Error('404'))
    await api.loadConfig()
    expect(m.logWarn).toHaveBeenCalled()
    expect(api.loading.value).toBe(false)
    expect(api.cfg.value.template).toBe(defaultTpl)
  })

  it('saveConfig：编译错误时拦截；正常时保存并通知', async () => {
    const { api } = harness()
    await flushPromises()
    api.cfg.value.template = '{"a": ${value'
    await api.saveConfig()
    expect(ElMessage.error).toHaveBeenCalledWith('模板编译失败，无法保存')
    expect(m.saveUplinkConfig).not.toHaveBeenCalled()

    api.cfg.value.template = '{"a": ${value}}'
    await api.saveConfig()
    expect(m.saveUplinkConfig).toHaveBeenCalledTimes(1)
    expect(ElNotification.success).toHaveBeenCalledWith(expect.objectContaining({ title: '已保存' }))
    expect(api.saving.value).toBe(false)
  })

  it('handleImport：缺 template 报错；合法 JSON 导入并重载；非法 JSON 提示解析失败', async () => {
    const { api } = harness()
    await flushPromises()

    const makeEvent = (text: string) =>
      ({ target: { files: [{ text: async () => text }], value: 'f.json' } }) as unknown as Event

    await api.handleImport(makeEvent('{"enabled":true}'))
    expect(ElMessage.warning).toHaveBeenCalledWith('文件格式不正确：缺少 template 字符串字段')
    expect(m.importTemplate).not.toHaveBeenCalled()

    m.getUplinkConfig.mockClear()
    await api.handleImport(makeEvent('{"template":"${value}"}'))
    expect(m.importTemplate).toHaveBeenCalledWith({ template: '${value}', enabled: true })
    expect(m.getUplinkConfig).toHaveBeenCalled() // 导入后 loadConfig 重载

    await api.handleImport(makeEvent('{bad json'))
    expect(ElMessage.warning).toHaveBeenCalledWith(expect.stringContaining('JSON 解析失败'))
  })

  it('goTester 跳转调试页', () => {
    const { api } = harness()
    api.goTester()
    expect(m.push).toHaveBeenCalledWith('/uplink?tab=uplink-tester')
  })

  it('doExportTemplate 导出并下载', async () => {
    const { api } = harness()
    await flushPromises()
    await api.doExportTemplate()
    expect(m.exportTemplate).toHaveBeenCalledTimes(1)
    expect(ElMessage.success).toHaveBeenCalledWith('模板已导出为 JSON 文件')
    expect(api.exporting.value).toBe(false)
  })
})