import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'
import ImportOutcome from '@/components/ImportOutcome.vue'

/**
 * D1：参数配置 › 导入导出行为测试。
 *
 * 这一页是唯一能「一次覆盖整机配置」的入口，做错了就是把一台现场的配置冲掉。
 * 钉住的核心是**导入的两步化**：解析 → 预览 → 确认才提交；关掉预览必须释放待导入内容。
 * 同时钉住「工程文件导入」与「整机配置导入」的差异：前者是子集、没有 diff 预览。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('@/api/config', () => ({ exportConfig: vi.fn(), importConfig: vi.fn() }))
vi.mock('@/api/project', () => ({ exportProject: vi.fn(), importProject: vi.fn() }))
vi.mock('@/utils/exportCsv', () => ({ triggerDownload: vi.fn() }))

import ConfigIo from './ConfigIo.vue'
import { exportConfig, importConfig } from '@/api/config'
import { exportProject, importProject } from '@/api/project'
import { triggerDownload } from '@/utils/exportCsv'

/** 预览对话框替换为纯道具桩：内部要跑 configDiff，只关心「传了哪份配置进去」 */
const PreviewStub = {
  name: 'ConfigChangePreview',
  props: {
    modelValue: { type: Boolean, default: false },
    config: { type: Object, default: null },
    parseError: { type: Boolean, default: false },
    confirmText: { type: String, default: '' },
  },
  emits: ['save', 'update:modelValue'],
  template: '<div class="preview-stub" />',
}

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(ConfigIo, { global: { stubs: { ConfigChangePreview: PreviewStub } } })
}

async function clickButton(wrapper: ReturnType<typeof mountPage>, text: string, index = 0) {
  const btns = wrapper.findAll('button').filter((b) => b.text() === text)
  expect(btns[index], `找不到按钮「${text}」`).toBeTruthy()
  await btns[index]!.trigger('click')
  await settle()
}

function configJson() {
  return JSON.stringify({ buses: [{ port_name: 'COM3', protocol: 'modbus' }], devices: [], points: [] }, null, 2)
}

describe('ConfigIo 配置导入导出', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(exportConfig).mockResolvedValue({ buses: [{ port_name: 'COM3' }], devices: [], points: [] } as never)
    vi.mocked(importConfig).mockResolvedValue(undefined as never)
    vi.mocked(exportProject).mockResolvedValue({ buses: [], north: {}, vars: [] } as never)
    vi.mocked(importProject).mockResolvedValue(undefined as never)
    Object.assign(navigator, { clipboard: { writeText: vi.fn() } })
  })

  it('初始状态：没有导出内容时复制与下载都禁用', async () => {
    const wrapper = mountPage()
    const copy = wrapper.findAll('button').filter((b) => b.text() === '复制到剪贴板')[0]!
    const download = wrapper.findAll('button').filter((b) => b.text() === '下载文件')[0]!

    expect(copy.attributes('disabled')).toBeDefined()
    expect(download.attributes('disabled')).toBeDefined()
    expect((wrapper.find('textarea[aria-label="导出的配置 JSON"]').element as HTMLTextAreaElement).value).toBe('')
  })

  it('导出 JSON：格式化回显并提示凭据已脱敏', async () => {
    const wrapper = mountPage()
    await clickButton(wrapper, '导出 JSON')

    const textarea = wrapper.find('textarea[aria-label="导出的配置 JSON"]').element as HTMLTextAreaElement
    expect(textarea.value).toContain('"port_name": "COM3"')
    expect(textarea.value.startsWith('{')).toBe(true)
    expect(ElMessage.success).toHaveBeenCalledWith('配置已导出（凭据字段已脱敏，迁移到新网关需重新填写）')
  })

  it('导出失败：给出可读原因，且不留下半份内容', async () => {
    vi.mocked(exportConfig).mockRejectedValue(new Error('config service 500'))
    const wrapper = mountPage()
    await clickButton(wrapper, '导出 JSON')

    expect(ElMessage.error).toHaveBeenCalledWith('导出失败：config service 500')
    expect((wrapper.find('textarea[aria-label="导出的配置 JSON"]').element as HTMLTextAreaElement).value).toBe('')
  })

  it('下载文件按 gateway-config 前缀命名，复制走剪贴板并提示', async () => {
    const wrapper = mountPage()
    await clickButton(wrapper, '导出 JSON')
    await clickButton(wrapper, '下载文件')
    expect(vi.mocked(triggerDownload).mock.calls[0]![1]).toMatch(/^gateway-config-\d+\.json$/)

    await clickButton(wrapper, '复制到剪贴板')
    expect(navigator.clipboard.writeText).toHaveBeenCalledOnce()
    expect(ElMessage.success).toHaveBeenCalledWith('已复制到剪贴板')
  })

  it('导入不再一点就覆盖：合法 JSON 只打开预览，不调用导入接口', async () => {
    const wrapper = mountPage()
    await wrapper.find('textarea[aria-label="待导入的配置 JSON"]').setValue(configJson())
    await clickButton(wrapper, '校验并导入')

    expect(importConfig).not.toHaveBeenCalled()
    const preview = wrapper.findComponent({ name: 'ConfigChangePreview' })
    expect(preview.props('modelValue')).toBe(true)
    expect(preview.props('config')).toMatchObject({ buses: [{ port_name: 'COM3', protocol: 'modbus' }] })
  })

  it('预览里确认后才真正提交，并给出成功提示', async () => {
    const wrapper = mountPage()
    await wrapper.find('textarea[aria-label="待导入的配置 JSON"]').setValue(configJson())
    await clickButton(wrapper, '校验并导入')

    wrapper.findComponent({ name: 'ConfigChangePreview' }).vm.$emit('save')
    await settle()

    expect(importConfig).toHaveBeenCalledOnce()
    expect(ElMessage.success).toHaveBeenCalledWith('配置导入成功')
    expect(wrapper.findAllComponents(ImportOutcome)[0]!.props('success')).toBe(true)
    // 提交后预览关闭、待导入内容释放（v-if 直接卸载）
    expect(wrapper.findComponent({ name: 'ConfigChangePreview' }).exists()).toBe(false)
  })

  it('关闭预览会丢弃已解析的配置（不能留一份悬空的待导入内容）', async () => {
    const wrapper = mountPage()
    await wrapper.find('textarea[aria-label="待导入的配置 JSON"]').setValue(configJson())
    await clickButton(wrapper, '校验并导入')
    expect(wrapper.findComponent({ name: 'ConfigChangePreview' }).props('config')).not.toBeNull()

    wrapper.findComponent({ name: 'ConfigChangePreview' }).vm.$emit('update:modelValue', false)
    await settle()

    expect(wrapper.findComponent({ name: 'ConfigChangePreview' }).exists()).toBe(false)
    expect(importConfig).not.toHaveBeenCalled()
  })

  it('非法 JSON 绝不会进入导入流程：不开预览、不调接口，并给出解析原因', async () => {
    const wrapper = mountPage()
    await wrapper.find('textarea[aria-label="待导入的配置 JSON"]').setValue('{ not json ')
    await clickButton(wrapper, '校验并导入')
    // 表单校验是异步的：错误必须落到输入框下方，用轮询等待而不是固定 sleep。
    // 文案由 utils/jsonTextValidator 统一提供（「XX格式错误，无法解析：…」）
    await vi.waitFor(() => expect(wrapper.text()).toContain('待导入的配置 JSON格式错误，无法解析'))

    expect(importConfig).not.toHaveBeenCalled()
    expect(wrapper.findComponent({ name: 'ConfigChangePreview' }).exists()).toBe(false)
  })

  it('导入接口失败：结果面板显示后端原因，且不谎报成功', async () => {
    vi.mocked(importConfig).mockRejectedValue(new Error('配置校验未通过：点位重复'))
    const wrapper = mountPage()
    await wrapper.find('textarea[aria-label="待导入的配置 JSON"]').setValue(configJson())
    await clickButton(wrapper, '校验并导入')
    wrapper.findComponent({ name: 'ConfigChangePreview' }).vm.$emit('save')
    // 提交是「校验 → 接口 → 渲染结果」的异步链，轮询等结果面板拿到失败原因再断言
    await vi.waitFor(() =>
      expect(wrapper.findAllComponents(ImportOutcome)[0]!.props('error')).toBe('配置校验未通过：点位重复'),
    )

    const outcome = wrapper.findAllComponents(ImportOutcome)[0]!
    expect(outcome.props('success')).toBe(false)
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('工程文件导入是子集写入：不走 diff 预览，校验通过即提交', async () => {
    const wrapper = mountPage()
    await wrapper.find('textarea[aria-label="待导入的工程文件 JSON"]').setValue(JSON.stringify({ buses: [], north: {}, vars: [] }))
    await clickButton(wrapper, '校验并导入', 1)

    // 校验是异步的：接口调用发生在 validate 之后，轮询等它真正提交
    await vi.waitFor(() => expect(importProject).toHaveBeenCalledOnce())
    expect(wrapper.findComponent({ name: 'ConfigChangePreview' }).exists()).toBe(false)
    expect(ElMessage.success).toHaveBeenCalledWith('工程文件导入成功')
  })

  it('工程文件导入失败时结果面板如实报错', async () => {
    vi.mocked(importProject).mockRejectedValue(new Error('工程文件版本不兼容'))
    const wrapper = mountPage()
    await wrapper.find('textarea[aria-label="待导入的工程文件 JSON"]').setValue(JSON.stringify({ buses: [], north: {}, vars: [] }))
    await clickButton(wrapper, '校验并导入', 1)

    await vi.waitFor(() =>
      expect(wrapper.findAllComponents(ImportOutcome)[1]!.props('error')).toBe('工程文件版本不兼容'),
    )
    const outcomes = wrapper.findAllComponents(ImportOutcome)
    expect(outcomes[1]!.props('success')).toBe(false)
  })

  it('工程文件导出与下载使用 gateway-project 前缀', async () => {
    const wrapper = mountPage()
    await clickButton(wrapper, '导出工程文件')

    const textarea = wrapper.find('textarea[aria-label="导出的工程文件 JSON"]').element as HTMLTextAreaElement
    expect(textarea.value).toContain('"north"')
    expect(ElMessage.success).toHaveBeenCalledWith('工程文件已导出')

    await clickButton(wrapper, '下载文件', 1)
    expect(vi.mocked(triggerDownload).mock.calls[0]![1]).toMatch(/^gateway-project-\d+\.json$/)
  })
})
