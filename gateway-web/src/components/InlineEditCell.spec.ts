import { describe, it, expect, beforeEach, vi } from 'vitest'
import { flushPromises, mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import InlineEditCell from './InlineEditCell.vue'

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { warning: vi.fn(), error: vi.fn(), success: vi.fn(), info: vi.fn() } }
})

import { ElMessage } from 'element-plus'

/** 双击进入编辑态，返回输入框 */
async function enterEdit(wrapper: ReturnType<typeof mountCell>) {
  await wrapper.get('.iec-view').trigger('dblclick')
  await nextTick()
  return wrapper.get('input')
}

/** Enter 提交内部是异步写请求，必须等 promise 落地再断言编辑态 */
async function pressEnter(input: { trigger: (event: string, options?: Record<string, unknown>) => Promise<void> }) {
  await input.trigger('keydown', { key: 'Enter' })
  await flushPromises()
}

function mountCell(props: Record<string, unknown> = {}) {
  return mount(InlineEditCell, {
    props: {
      value: '旧值',
      label: '名称',
      save: vi.fn().mockResolvedValue(true),
      ...props,
    },
  })
}

describe('InlineEditCell', () => {
  beforeEach(() => {
    vi.mocked(ElMessage.warning).mockClear()
  })

  it('展示态渲染文本并提示双击可改', () => {
    const wrapper = mountCell()
    const view = wrapper.get('.iec-view')
    expect(view.text()).toBe('旧值')
    expect(view.attributes('title')).toBe('双击修改名称')
    expect(view.attributes('tabindex')).toBe('0')
  })

  it('空值显示占位符而不是空白单元格', () => {
    const wrapper = mountCell({ value: null })
    expect(wrapper.get('.iec-view').text()).toBe('—')
    expect(wrapper.get('.iec-view').classes()).toContain('is-empty')
  })

  it('双击进入编辑，Enter 提交并退出编辑', async () => {
    const save = vi.fn().mockResolvedValue(true)
    const wrapper = mountCell({ save })

    const input = await enterEdit(wrapper)
    await input.setValue('  新值  ')
    await pressEnter(input)

    expect(save).toHaveBeenCalledTimes(1)
    // 首尾空白不应写进配置
    expect(save).toHaveBeenCalledWith('新值')
    expect(wrapper.find('.iec-view').exists()).toBe(true)
    expect(wrapper.find('input').exists()).toBe(false)
  })

  it('Esc 取消不提交，恢复原值', async () => {
    const save = vi.fn().mockResolvedValue(true)
    const wrapper = mountCell({ save })

    const input = await enterEdit(wrapper)
    await input.setValue('改了一半')
    await input.trigger('keydown', { key: 'Escape' })

    expect(save).not.toHaveBeenCalled()
    expect(wrapper.get('.iec-view').text()).toBe('旧值')
  })

  it('保存失败保持编辑态，用户输入不丢', async () => {
    const save = vi.fn().mockResolvedValue(false)
    const wrapper = mountCell({ save })

    const input = await enterEdit(wrapper)
    await input.setValue('新值')
    await pressEnter(input)

    expect(save).toHaveBeenCalledWith('新值')
    const stillEditing = wrapper.get('input')
    expect((stillEditing.element as HTMLInputElement).value).toBe('新值')
  })

  it('必填项清空时不提交并保持编辑态', async () => {
    const save = vi.fn().mockResolvedValue(true)
    const wrapper = mountCell({ save })

    const input = await enterEdit(wrapper)
    await input.setValue('   ')
    await pressEnter(input)

    expect(save).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalledWith(expect.stringContaining('不能为空'))
    expect(wrapper.find('input').exists()).toBe(true)
  })

  it('值未变化时不发请求，直接退出编辑', async () => {
    const save = vi.fn().mockResolvedValue(true)
    const wrapper = mountCell({ save })

    const input = await enterEdit(wrapper)
    await pressEnter(input)

    expect(save).not.toHaveBeenCalled()
    expect(wrapper.find('input').exists()).toBe(false)
  })

  it('数字列拒绝非整数与越界值', async () => {
    const save = vi.fn().mockResolvedValue(true)
    const wrapper = mountCell({ value: 1, label: '从站号', type: 'number', min: 1, max: 247, save })

    let input = await enterEdit(wrapper)
    await input.setValue('1.5')
    await pressEnter(input)
    expect(save).not.toHaveBeenCalled()

    input = wrapper.get('input')
    await input.setValue('300')
    await pressEnter(input)
    expect(save).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalledWith(expect.stringContaining('1–247'))

    await input.setValue('247')
    await pressEnter(input)
    expect(save).toHaveBeenCalledWith(247)
  })

  it('disabled 时不能进入编辑', async () => {
    const wrapper = mountCell({ disabled: true })
    await wrapper.get('.iec-view').trigger('dblclick')
    await nextTick()
    expect(wrapper.find('input').exists()).toBe(false)
  })
})
