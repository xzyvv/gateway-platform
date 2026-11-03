/**
 * 单步调试自定义序列（localStorage）单元测试
 *
 * 该模块是「纯前端 A 档」的存储与校验层：序列只存浏览器，
 * 因此这里重点验证「首次默认值 / 落盘往返 / 步骤编排 / 导入导出容错」。
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'
import { scopedSet } from '@/utils/gatewayStorage'

/**
 * 模拟后端 `GET /api/protocol-tools/packet-editor/presets` 的返回。
 * 前端已不再内置预置副本（P3-4），默认序列的步骤由后端预置注入。
 */
const TEST_PRESETS = [
  { name: '读保持寄存器(0-1)', hex: '01 03 00 00 00 02' },
  { name: '读输入寄存器(0-3)', hex: '01 04 00 00 00 04' },
  { name: '读线圈(0-7)', hex: '01 01 00 00 00 08' },
  { name: '写单寄存器(5=100)', hex: '01 06 00 05 00 64' },
]

/** 重置模块缓存后重新加载，模拟「刷新页面」从 localStorage 恢复 */
async function freshStore() {
  vi.resetModules()
  const m = await import('./useStepSequences')
  m.seedDefaultSequence(TEST_PRESETS)
  return m
}

describe('useStepSequences', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('无本地数据时生成默认序列，注入后端预置后含 4 条步骤', async () => {
    const m = await freshStore()
    const { sequences, activeSequence } = m.useStepSequences()
    expect(sequences.value).toHaveLength(1)
    expect(activeSequence.value?.name).toBe('默认序列')
    expect(activeSequence.value?.steps).toHaveLength(4)
    expect(activeSequence.value?.steps[0]!.hex).toBe('01 03 00 00 00 02')
  })

  it('编辑后落盘，重新加载仍保留', async () => {
    const m = await freshStore()
    const store = m.useStepSequences()
    store.renameSequence(store.activeId.value, '电表上电检查')
    store.addStep({ name: '读序列号', hex: '01 03 00 20 00 02' })
    await nextTick()

    const reloaded = (await freshStore()).useStepSequences()
    expect(reloaded.activeSequence.value?.name).toBe('电表上电检查')
    expect(reloaded.activeSequence.value?.steps.at(-1)?.name).toBe('读序列号')
  })

  it('新建/删除序列，删除最后一个时回退为空默认序列', async () => {
    const m = await freshStore()
    const store = m.useStepSequences()
    const created = store.createSequence('第二序列', 'COM3')
    expect(store.sequences.value).toHaveLength(2)
    expect(store.activeId.value).toBe(created.id)

    store.deleteSequence(created.id)
    expect(store.sequences.value).toHaveLength(1)

    store.deleteSequence(store.activeId.value)
    expect(store.sequences.value).toHaveLength(1)
    expect(store.activeSequence.value?.steps).toHaveLength(0)
  })

  it('步骤可上移/下移/复制/删除，越界移动被夹紧', async () => {
    const m = await freshStore()
    const store = m.useStepSequences()
    const seq = store.activeSequence.value!
    const first = seq.steps[0]!
    const second = seq.steps[1]!

    store.moveStep(first.id, -1)
    expect(seq.steps[0]!.id).toBe(first.id)
    store.moveStep(first.id, 1)
    expect(seq.steps[1]!.id).toBe(first.id)

    const copy = store.duplicateStep(second.id)
    expect(copy?.name).toContain('副本')
    store.removeStep(second.id)
    expect(seq.steps.some((s) => s.id === second.id)).toBe(false)
  })

  it('HEX 校验与超时收窄', async () => {
    const m = await freshStore()
    expect(m.isValidHex('01 03 00 00 00 02')).toBe(true)
    expect(m.isValidHex('0103000')).toBe(false)
    expect(m.isValidHex('01 03 0G')).toBe(false)
    expect(m.isValidHex('   ')).toBe(false)
    expect(m.normalizeHex('  01   03  ')).toBe('01 03')
    expect(m.clampTimeout(10)).toBe(100)
    expect(m.clampTimeout(999_999)).toBe(10_000)
    expect(m.clampTimeout(Number.NaN)).toBe(1_500)
  })

  it('导入 JSON：合法步骤保留，非法步骤丢弃', async () => {
    const m = await freshStore()
    const store = m.useStepSequences()
    const text = JSON.stringify({
      name: '厂商调试流程',
      bus: 'COM3',
      steps: [
        { name: '读设备ID', hex: '01 03 00 00 00 02', timeoutMs: 800 },
        { name: '坏帧', hex: '01 03 0' },
      ],
    })

    const result = store.importFromJson(text)
    expect(result.ok).toBe(true)
    const imported = store.sequences.value.at(-1)!
    expect(imported.name).toBe('厂商调试流程')
    expect(imported.bus).toBe('COM3')
    expect(imported.steps).toHaveLength(1)
    expect(imported.steps[0]!.timeoutMs).toBe(800)
  })

  it('导入非法 JSON 返回失败；导出可再次导入', async () => {
    const m = await freshStore()
    const store = m.useStepSequences()
    expect(store.importFromJson('{bad').ok).toBe(false)

    const before = store.sequences.value.length
    expect(store.importFromJson(store.toJson()).ok).toBe(true)
    expect(store.sequences.value.length).toBe(before + 1)
  })

  it('localStorage 脏数据回退为默认序列', async () => {
    scopedSet('gw_step_debug_sequences', '{oops')
    const m = await freshStore()
    const store = m.useStepSequences()
    expect(store.sequences.value).toHaveLength(1)
    expect(store.sequences.value[0]!.steps).toHaveLength(4)
  })

  describe('undo / redo', () => {
    it('初始无可撤销/重做', async () => {
      const m = await freshStore()
      const store = m.useStepSequences()
      expect(store.canUndo.value).toBe(false)
      expect(store.canRedo.value).toBe(false)
    })

    it('删除步骤后可撤销恢复，再重做可再次删除', async () => {
      const m = await freshStore()
      const store = m.useStepSequences()
      const seq = store.activeSequence.value!
      const beforeLen = seq.steps.length
      const victim = seq.steps[0]!

      store.removeStep(victim.id)
      expect(store.activeSequence.value!.steps).toHaveLength(beforeLen - 1)
      expect(store.canUndo.value).toBe(true)
      expect(store.canRedo.value).toBe(false)

      store.undo()
      // restore 替换了整个 sequences 数组，需重新取 activeSequence
      expect(store.activeSequence.value!.steps).toHaveLength(beforeLen)
      expect(store.activeSequence.value!.steps.some((s) => s.id === victim.id)).toBe(true)
      expect(store.canRedo.value).toBe(true)

      store.redo()
      expect(store.activeSequence.value!.steps).toHaveLength(beforeLen - 1)
      expect(store.activeSequence.value!.steps.some((s) => s.id === victim.id)).toBe(false)
    })

    it('新建序列后撤销可删除该序列，重做可恢复', async () => {
      const m = await freshStore()
      const store = m.useStepSequences()
      const beforeLen = store.sequences.value.length
      const created = store.createSequence('待撤销', '')

      expect(store.sequences.value).toHaveLength(beforeLen + 1)
      store.undo()
      expect(store.sequences.value).toHaveLength(beforeLen)
      expect(store.sequences.value.some((s) => s.id === created.id)).toBe(false)

      store.redo()
      expect(store.sequences.value).toHaveLength(beforeLen + 1)
      expect(store.sequences.value.some((s) => s.id === created.id)).toBe(true)
    })

    it('新分支提交后清空 redo 栈', async () => {
      const m = await freshStore()
      const store = m.useStepSequences()
      const seq = store.activeSequence.value!
      store.removeStep(seq.steps[0]!.id)
      store.undo()
      expect(store.canRedo.value).toBe(true)

      // 撤销后再做一次新的结构变更，redo 栈应被清空
      store.addStep({ name: '新分支', hex: '01 03 00 00 00 01' })
      expect(store.canRedo.value).toBe(false)
    })

    it('updateStep（逐键编辑）不入历史栈', async () => {
      const m = await freshStore()
      const store = m.useStepSequences()
      const seq = store.activeSequence.value!
      const step = seq.steps[0]!
      store.updateStep(step.id, { name: '改个名' })
      expect(store.canUndo.value).toBe(false)
    })
  })
})
