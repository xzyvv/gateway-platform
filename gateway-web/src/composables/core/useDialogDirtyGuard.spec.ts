import { describe, it, expect, vi, beforeEach } from 'vitest'
import { ref, nextTick, type Ref } from 'vue'
import { ElMessageBox } from 'element-plus'
import { useDialogDirtyGuard } from './useDialogDirtyGuard'
import { isScopeDirty } from '../../utils/dirtyGuard'

vi.mock('element-plus', () => ({
  ElMessageBox: { confirm: vi.fn() },
}))

/** 打开弹窗并等 post-flush 基线采集完成 */
async function open(visible: Ref<boolean>) {
  visible.value = true
  await nextTick()
  await nextTick()
}

describe('useDialogDirtyGuard', () => {
  const confirmMock = ElMessageBox.confirm as ReturnType<typeof vi.fn>
  let visible: Ref<boolean>
  let form: Ref<Record<string, unknown>>

  beforeEach(() => {
    vi.clearAllMocks()
    visible = ref(false)
    form = ref<Record<string, unknown>>({ name: '', count: 0 })
  })

  it('无改动时 confirmClose 直接放行，不弹确认框', async () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    await open(visible)
    expect(await guard.confirmClose()).toBe(true)
    expect(confirmMock).not.toHaveBeenCalled()
  })

  it('有未保存修改：用户确认放弃 → 放行并清脏', async () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    await open(visible)
    form.value = { name: '改过的', count: 0 }
    confirmMock.mockResolvedValueOnce('confirm')
    expect(await guard.confirmClose()).toBe(true)
    expect(confirmMock).toHaveBeenCalledTimes(1)
    // 确认放弃后视为干净：再次关闭不再弹
    expect(await guard.confirmClose()).toBe(true)
    expect(confirmMock).toHaveBeenCalledTimes(1)
  })

  it('有未保存修改：用户点「继续编辑」→ 拦截关闭，弹窗保持打开', async () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    await open(visible)
    form.value = { name: 'x', count: 0 }
    confirmMock.mockRejectedValueOnce('cancel')
    expect(await guard.confirmClose()).toBe(false)
    expect(visible.value).toBe(true)
  })

  it('close()：确认后关闭弹窗；拦截后保持打开', async () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    await open(visible)
    form.value = { name: 'x', count: 0 }

    confirmMock.mockRejectedValueOnce('cancel')
    await guard.close()
    expect(visible.value).toBe(true)

    confirmMock.mockResolvedValueOnce('confirm')
    await guard.close()
    expect(visible.value).toBe(false)
  })

  it('beforeClose：确认后调用 done，拦截后不调用', async () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    await open(visible)
    form.value = { name: 'x', count: 0 }

    const done = vi.fn()
    confirmMock.mockRejectedValueOnce('cancel')
    await guard.beforeClose(done)
    expect(done).not.toHaveBeenCalled()

    confirmMock.mockResolvedValueOnce('confirm')
    await guard.beforeClose(done)
    expect(done).toHaveBeenCalledTimes(1)
  })

  it('重新打开后基线重取：上次的修改不再被误判为脏', async () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    await open(visible)
    form.value = { name: '上次没保存的', count: 0 }
    confirmMock.mockResolvedValueOnce('confirm')
    await guard.close()

    await open(visible)
    expect(guard.isDirty()).toBe(false)
    expect(confirmMock).toHaveBeenCalledTimes(1)
  })

  it('markClean()：异步填充完成后重取基线', async () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    await open(visible)
    // 模拟 @open 之后异步回填编辑数据
    form.value = { name: '回填数据', count: 3 }
    expect(guard.isDirty()).toBe(true)
    guard.markClean()
    expect(guard.isDirty()).toBe(false)
  })

  it('弹窗未打开时不判脏', () => {
    const guard = useDialogDirtyGuard(visible, () => form.value)
    form.value = { name: 'x', count: 0 }
    expect(guard.isDirty()).toBe(false)
  })

  describe('scope 桥接（弹窗脏状态同步路由级注册表）', () => {
    it('弹窗内改字段 → scope 标脏；改回基线 → 清脏', async () => {
      const scope = '/test-bridge-1'
      useDialogDirtyGuard(visible, () => form.value, { scope })
      await open(visible)
      expect(isScopeDirty(scope)).toBe(false)

      form.value = { name: '改过的', count: 0 }
      await nextTick()
      expect(isScopeDirty(scope)).toBe(true)

      form.value = { name: '', count: 0 }
      await nextTick()
      expect(isScopeDirty(scope)).toBe(false)
    })

    it('弹窗关闭后 scope 不再脏（弹窗开着跳路由被拦、关窗后放行）', async () => {
      const scope = '/test-bridge-2'
      const guard = useDialogDirtyGuard(visible, () => form.value, { scope })
      await open(visible)
      form.value = { name: 'x', count: 0 }
      await nextTick()
      expect(isScopeDirty(scope)).toBe(true)

      // 用户确认放弃 → 弹窗关闭 → 路由级脏标记同步清除
      confirmMock.mockResolvedValueOnce('confirm')
      await guard.close()
      await nextTick()
      expect(isScopeDirty(scope)).toBe(false)
    })

    it('不传 scope 时不注册路由级脏标记（行为与旧版一致）', async () => {
      const guard = useDialogDirtyGuard(visible, () => form.value)
      await open(visible)
      form.value = { name: 'x', count: 0 }
      await nextTick()
      expect(guard.isDirty()).toBe(true)
      // 任意 scope 都不应被污染
      expect(isScopeDirty('/test-bridge-absent')).toBe(false)
    })
  })
})
