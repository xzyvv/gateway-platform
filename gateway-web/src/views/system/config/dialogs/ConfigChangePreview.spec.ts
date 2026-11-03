// ConfigChangePreview 回归测试：覆盖后端 config_diff 契约
// { changed_fields: string[]; has_changes: boolean } 的识别与渲染。
//
// 背景：修复前组件只认识 unified-diff 字符串 / 行数组，遇到该对象会退化成
// 「无变更」；修复后 previewDiff 里的类型守卫 isChangedFieldsResult 识别该契约，
// 并以 has_changes === true || changed_fields.length > 0 取并集判定有变更。
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import ConfigChangePreview from './ConfigChangePreview.vue'
import type { GatewayConfig } from '@/api/types'

const { configDiffMock } = vi.hoisted(() => ({ configDiffMock: vi.fn() }))

// 仅替换 configDiff，组件其余依赖保持真实实现
vi.mock('@/api/config', () => ({
  configDiff: (...args: unknown[]) => configDiffMock(...args),
}))

// ElMessage 走 mock，避免真实提示依赖；其余 element-plus 保持真实（含 el-dialog / el-tag）
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { error: vi.fn(), success: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

const allWrappers: ReturnType<typeof mount>[] = []
afterEach(() => {
  for (const w of allWrappers) w.unmount()
  allWrappers.length = 0
  document.body.innerHTML = ''
  vi.clearAllMocks()
})

/** 以「先关闭再打开」触发 watch(visible) → previewDiff（watch 非 immediate） */
async function mountAndOpen() {
  const wrapper = mount(ConfigChangePreview, {
    props: {
      modelValue: false,
      config: {} as unknown as GatewayConfig,
      parseError: false,
    },
    attachTo: document.body,
  })
  allWrappers.push(wrapper)
  await nextTick()
  await wrapper.setProps({ modelValue: true })
  await flushPromises()
  await nextTick()
  return wrapper
}

describe('ConfigChangePreview.vue — config_diff 契约', () => {
  beforeEach(() => {
    configDiffMock.mockReset()
  })

  it('changed_fields 非空：渲染「N 处字段变更」与中文标签，不出现「无变更」', async () => {
    configDiffMock.mockResolvedValue({ changed_fields: ['buses', 'mqtt'], has_changes: true })
    const wrapper = await mountAndOpen()
    const text = wrapper.text()
    expect(text).toContain('2 处字段变更')
    expect(text).toContain('总线配置')
    expect(text).toContain('MQTT 通道')
    expect(text).not.toContain('无变更')
  })

  it('changed_fields 为空且 has_changes=false：渲染「无变更」，不出现「处字段变更」', async () => {
    configDiffMock.mockResolvedValue({ changed_fields: [], has_changes: false })
    const wrapper = await mountAndOpen()
    const text = wrapper.text()
    expect(text).toContain('无变更')
    expect(text).not.toContain('处字段变更')
  })

  it('契约防御：changed_fields 非空但 has_changes=false，仍判为有变更（取并集）', async () => {
    configDiffMock.mockResolvedValue({ changed_fields: ['buses'], has_changes: false })
    const wrapper = await mountAndOpen()
    const text = wrapper.text()
    expect(text).toContain('1 处字段变更')
    expect(text).toContain('总线配置')
  })
})
