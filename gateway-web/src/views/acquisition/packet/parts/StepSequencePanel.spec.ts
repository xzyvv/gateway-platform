import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：单步调试 › 自定义序列面板（StepSequencePanel）行为测试。
 *
 * 序列是现场交接报文用的：误删一步/换错总线都会让下一次下发打到别的设备。
 * 钉住的核心：
 * - 执行中禁止一切编辑（新建/删步/改内容/导入），只能在「中止」后继续；
 * - 任何结构变更都要通知主组件清空执行进度，避免旧结果对应新序列；
 * - 删除必须二次确认、取消即什么都不做；
 * - 导入的坏文件/超大文件要明确拦下，且不产生半截序列。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('@/utils/confirm', () => ({ confirmDelete: (...args: unknown[]) => confirmDelete(...args) }))
vi.mock('@/utils/exportCsv', () => ({ triggerDownload: (...args: unknown[]) => triggerDownload(...args) }))
vi.mock('@/composables/acquisition/useDebugTargets', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  if (!h.selectedBus.current) h.selectedBus.current = ref('')
  return { useSharedDebugTarget: () => ({ selectedBus: h.selectedBus.current }) }
})
vi.mock('@/composables/acquisition/useStepSequences', async () => {
  const { ref, computed } = await vi.importActual<typeof import('vue')>('vue')
  const s = h.state
  if (!s.sequences.current) {
    s.sequences.current = ref([])
    s.activeId.current = ref('')
    s.canUndo.current = ref(false)
    s.canRedo.current = ref(false)
  }
  return {
    MIN_TIMEOUT_MS: 100,
    MAX_TIMEOUT_MS: 10000,
    useStepSequences: () => ({
      sequences: s.sequences.current,
      activeId: s.activeId.current,
      activeSequence: computed(
        () => s.sequences.current.value.find((x) => x.id === s.activeId.current.value) ?? null,
      ),
      canUndo: s.canUndo.current,
      canRedo: s.canRedo.current,
      undo: s.undo,
      redo: s.redo,
      selectSequence: s.selectSequence,
      createSequence: s.createSequence,
      deleteSequence: s.deleteSequence,
      renameSequence: s.renameSequence,
      setSequenceBus: s.setSequenceBus,
      addStep: s.addStep,
      toJson: s.toJson,
      importFromJson: s.importFromJson,
    }),
  }
})

const confirmDelete = vi.hoisted(() => vi.fn())
const triggerDownload = vi.hoisted(() => vi.fn())

/** 测试用序列结构：只关心 id/name/bus/steps.id，其余字段不参与断言 */
interface Seq {
  id: string
  name: string
  bus: string
  steps: Array<{ id: string }>
}

/** 一个「已执行成功」的步骤结果；组件只判断 outcomes[id] 是否存在 */
function outcome(patch: Partial<StepOutcome> = {}): StepOutcome {
  return {
    status: 'ok',
    requestHex: '',
    responseHex: '',
    parsed: '',
    latencyMs: 0,
    error: null,
    at: 1,
    ...patch,
  }
}

const h = vi.hoisted(() => ({
  selectedBus: { current: null as unknown as { value: string } },
  state: {
    sequences: { current: null as unknown as { value: Seq[] } },
    activeId: { current: null as unknown as { value: string } },
    canUndo: { current: null as unknown as { value: boolean } },
    canRedo: { current: null as unknown as { value: boolean } },
    undo: vi.fn(),
    redo: vi.fn(),
    selectSequence: vi.fn(),
    createSequence: vi.fn(),
    deleteSequence: vi.fn(),
    renameSequence: vi.fn(),
    setSequenceBus: vi.fn(),
    addStep: vi.fn(),
    toJson: vi.fn(),
    importFromJson: vi.fn(),
  },
}))

import StepSequencePanel from './StepSequencePanel.vue'
import StepSequenceToolbar from './StepSequenceToolbar.vue'
import type { StepOutcome } from './stepDebugTypes'

const PRESETS = [
  { name: '读保持寄存器', hex: '01 03 00 00 00 01' },
  { name: '读输入寄存器', hex: '01 04 00 00 00 01' },
]

async function settle() {
  await flushPromises()
  await nextTick()
}

function setSequence(seq: Seq) {
  h.state.sequences.current.value = [seq]
  h.state.activeId.current.value = seq.id
}

function mountPanel(props: Partial<{ outcomes: Record<string, StepOutcome>; running: boolean }> = {}) {
  return mount(StepSequencePanel, {
    props: {
      busOptions: [
        { label: 'COM1', value: 'COM1' },
        { label: 'COM2', value: 'COM2' },
      ],
      busLoading: false,
      outcomes: {},
      running: false,
      stopOnFail: false,
      runningStepId: '',
      presets: PRESETS,
      ...props,
    },
  })
}

function button(wrapper: ReturnType<typeof mountPanel>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim().includes(text))
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

function toolbar(wrapper: ReturnType<typeof mountPanel>) {
  return wrapper.findComponent(StepSequenceToolbar)
}

describe('StepSequencePanel 自定义序列', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    h.selectedBus.current.value = 'COM1'
    h.state.canUndo.current.value = false
    h.state.canRedo.current.value = false
    setSequence({ id: 'seq-1', name: '序列A', bus: 'COM1', steps: [] })
    confirmDelete.mockResolvedValue(true)
    h.state.addStep.mockReturnValue({ id: 'step-new' })
    h.state.importFromJson.mockReturnValue({ ok: true, message: '已导入 1 个序列' })
    h.state.createSequence.mockImplementation((name: string, bus: string) => {
      const seq = { id: 'seq-new', name: name.trim() || '新序列', bus, steps: [] }
      h.state.sequences.current.value = [...h.state.sequences.current.value, seq]
      h.state.activeId.current.value = seq.id
      return seq
    })
    h.state.deleteSequence.mockImplementation((id: string) => {
      h.state.sequences.current.value = h.state.sequences.current.value.filter((s) => s.id !== id)
      h.state.activeId.current.value = ''
    })
  })

  it('空序列时执行类按钮禁用，编辑入口可用', async () => {
    const wrapper = mountPanel()
    await settle()

    expect(button(wrapper, '单步执行').attributes('disabled')).toBeDefined()
    expect(button(wrapper, '自动执行剩余').attributes('disabled')).toBeDefined()
    expect(button(wrapper, '重置进度').attributes('disabled')).toBeDefined()
    expect(button(wrapper, '添加步骤').attributes('disabled')).toBeUndefined()
    expect(button(wrapper, '新建').attributes('disabled')).toBeUndefined()
    expect(button(wrapper, '导出').attributes('disabled')).toBeUndefined()
  })

  it('执行中禁止一切编辑与执行入口，只留「中止」', async () => {
    const wrapper = mountPanel({ running: true })
    await settle()

    for (const text of ['新建', '添加步骤']) {
      expect(button(wrapper, text).attributes('disabled')).toBeDefined()
    }
    expect(wrapper.find('button[aria-label="撤销"]').attributes('disabled')).toBeDefined()
    expect(button(wrapper, '单步执行').attributes('disabled')).toBeDefined()
    expect(button(wrapper, '中止').exists()).toBe(true)
  })

  it('新建序列：沿用当前总线，并通知主组件清进度、中止在跑的批次', async () => {
    const wrapper = mountPanel()
    await settle()
    await button(wrapper, '新建').trigger('click')
    await settle()

    expect(h.state.createSequence).toHaveBeenCalledWith('新序列', 'COM1')
    expect(wrapper.emitted('structure-changed')).toHaveLength(1)
    expect(wrapper.emitted('abort')).toHaveLength(1)
    expect(h.state.activeId.current.value).toBe('seq-new')
  })

  it('删除序列必须二次确认：取消时什么都不做', async () => {
    confirmDelete.mockResolvedValue(false)
    const wrapper = mountPanel()
    await settle()
    await button(wrapper, '删除').trigger('click')
    await settle()

    expect(confirmDelete).toHaveBeenCalledWith('序列A', { title: '删除序列' })
    expect(h.state.deleteSequence).not.toHaveBeenCalled()
    expect(wrapper.emitted('structure-changed')).toBeUndefined()
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('确认删除后才删除，并清空执行进度', async () => {
    const wrapper = mountPanel()
    await settle()
    await button(wrapper, '删除').trigger('click')
    await settle()

    expect(h.state.deleteSequence).toHaveBeenCalledWith('seq-1')
    expect(wrapper.emitted('structure-changed')).toHaveLength(1)
    expect(wrapper.emitted('abort')).toHaveLength(1)
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
  })

  it('重命名走工具栏提交，空名由存储层兜底成「未命名序列」', async () => {
    const wrapper = mountPanel()
    await settle()
    toolbar(wrapper).vm.$emit('rename-sequence', '新名字')
    await settle()

    expect(h.state.renameSequence).toHaveBeenCalledWith('seq-1', '新名字')
  })

  it('切换总线：先落共享目标，再写回序列，并清空旧结果', async () => {
    const wrapper = mountPanel()
    await settle()
    toolbar(wrapper).vm.$emit('bus-change', 'COM2')
    await settle()

    expect(h.selectedBus.current.value).toBe('COM2')
    expect(h.state.setSequenceBus).toHaveBeenCalledWith('seq-1', 'COM2')
    expect(wrapper.emitted('structure-changed')).toHaveLength(1)
  })

  it('切换序列时采用序列自带总线（换网关复用同一份序列）', async () => {
    setSequence({ id: 'seq-1', name: '序列A', bus: 'COM2', steps: [] })
    h.state.sequences.current.value = [
      { id: 'seq-1', name: '序列A', bus: 'COM2', steps: [] },
      { id: 'seq-2', name: '序列B', bus: '', steps: [] },
    ]
    const wrapper = mountPanel()
    await settle()
    toolbar(wrapper).vm.$emit('select-sequence', 'seq-1')
    await settle()

    expect(h.state.selectSequence).toHaveBeenCalledWith('seq-1')
    expect(h.selectedBus.current.value).toBe('COM2')
    expect(wrapper.emitted('structure-changed')).toHaveLength(1)
    expect(wrapper.emitted('abort')).toHaveLength(1)
  })

  it('添加空步骤与从预置添加都落到当前序列', async () => {
    const wrapper = mountPanel()
    await settle()

    await button(wrapper, '添加步骤').trigger('click')
    expect(h.state.addStep).toHaveBeenCalledWith()

    const dropdown = wrapper.findComponent({ name: 'ElDropdown' })
    dropdown.vm.$emit('command', PRESETS[0])
    expect(h.state.addStep).toHaveBeenCalledWith({ name: '读保持寄存器', hex: '01 03 00 00 00 01' })
  })

  it('步骤达到上限时明确提示而不是静默失败', async () => {
    h.state.addStep.mockReturnValue(null)
    const wrapper = mountPanel()
    await settle()
    await button(wrapper, '添加步骤').trigger('click')

    expect(ElMessage.warning).toHaveBeenCalledWith('步骤已达上限')
  })

  it('单步执行/自动执行只在还有未执行步骤时可用', async () => {
    const wrapper = mountPanel()
    await settle()
    await button(wrapper, '添加步骤').trigger('click')
    h.state.sequences.current.value = [
      { id: 'seq-1', name: '序列A', bus: 'COM1', steps: [{ id: 'step-new' }] },
    ]
    await settle()
    // outcomes 为空 → 还有待执行步骤
    await wrapper.setProps({ outcomes: {} })
    await settle()
    await button(wrapper, '单步执行').trigger('click')
    expect(wrapper.emitted('step-once')).toHaveLength(1)
    await button(wrapper, '自动执行剩余').trigger('click')
    expect(wrapper.emitted('run-rest')).toHaveLength(1)

    // 一步已有结果 → 没有待执行步骤
    await wrapper.setProps({ outcomes: { 'step-new': outcome() } })
    await settle()
    expect(button(wrapper, '单步执行').attributes('disabled')).toBeDefined()
    expect(button(wrapper, '自动执行剩余').attributes('disabled')).toBeDefined()
  })

  it('重置进度在零进度时禁用，有进度后可点', async () => {
    const wrapper = mountPanel()
    await settle()
    h.state.sequences.current.value = [
      { id: 'seq-1', name: '序列A', bus: 'COM1', steps: [{ id: 's1' }, { id: 's2' }] },
    ]
    await wrapper.setProps({ outcomes: { s1: outcome() } })
    await settle()

    expect(button(wrapper, '重置进度').attributes('disabled')).toBeUndefined()
    await button(wrapper, '重置进度').trigger('click')
    expect(wrapper.emitted('reset-progress')).toHaveLength(1)
  })

  it('导入非法 JSON：报错且不改动任何序列', async () => {
    h.state.importFromJson.mockReturnValue({ ok: false, message: 'JSON 解析失败，请确认文件内容' })
    const wrapper = mountPanel()
    await settle()
    const input = wrapper.find('input[type="file"]')
    Object.defineProperty(input.element, 'files', {
      value: [{ name: 'bad.json', size: 10, text: async () => '{ not json' }],
      configurable: true,
    })
    await input.trigger('change')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('JSON 解析失败，请确认文件内容')
    expect(wrapper.emitted('structure-changed')).toBeUndefined()
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('导入超过 1MB 的文件直接拦下，不读取内容', async () => {
    const wrapper = mountPanel()
    await settle()
    const text = vi.fn(async () => '{}')
    const input = wrapper.find('input[type="file"]')
    Object.defineProperty(input.element, 'files', {
      value: [{ name: 'big.json', size: 1024 * 1024 + 1, text }],
      configurable: true,
    })
    await input.trigger('change')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('文件过大（上限 1MB）')
    expect(text).not.toHaveBeenCalled()
    expect(h.state.importFromJson).not.toHaveBeenCalled()
  })

  it('导入合法序列：提示成功并清空执行进度', async () => {
    const wrapper = mountPanel()
    await settle()
    const input = wrapper.find('input[type="file"]')
    Object.defineProperty(input.element, 'files', {
      value: [{ name: 'seq.json', size: 100, text: async () => '{"name":"导入序列","steps":[]}' }],
      configurable: true,
    })
    await input.trigger('change')
    await settle()

    expect(h.state.importFromJson).toHaveBeenCalledOnce()
    expect(ElMessage.success).toHaveBeenCalledWith('已导入 1 个序列')
    expect(wrapper.emitted('structure-changed')).toHaveLength(1)
    expect(wrapper.emitted('abort')).toHaveLength(1)
  })

  it('导出文件名做安全清洗，避免非法字符写不进系统', async () => {
    setSequence({ id: 'seq-1', name: '带/斜杠 的名字', bus: 'COM1', steps: [] })
    h.state.toJson.mockReturnValue('{"name":"x"}')
    const wrapper = mountPanel()
    await settle()
    await button(wrapper, '导出').trigger('click')

    expect(triggerDownload).toHaveBeenCalledOnce()
    expect(triggerDownload.mock.calls[0]![1]).toBe('带_斜杠_的名字.json')
  })
})
