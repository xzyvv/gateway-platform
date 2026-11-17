import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { ElMessageBox } from 'element-plus'
import {
  __resetAllDirtyState,
  confirmDiscardDirty,
  isAnyDirty,
  isScopeDirty,
  registerDirtyGuard,
} from './dirtyGuard'

vi.mock('element-plus', () => ({
  ElMessageBox: { confirm: vi.fn() },
}))

/**
 * 未保存更改守卫是全仓「改动丢失保护」的唯一出口，此前零单测。
 *
 * 这里重点钉住两条容易改坏的行为：
 *  1. 取消返回 false 且**不清**任何脏标记（清错了等于静默丢数据）；
 *  2. 确认只清当前 scope，其他 tab 的未保存数据必须保留（Bug-W6）。
 */
describe('dirtyGuard', () => {
  const confirmMock = ElMessageBox.confirm as ReturnType<typeof vi.fn>

  beforeEach(() => {
    vi.clearAllMocks()
    __resetAllDirtyState()
  })

  afterEach(() => {
    __resetAllDirtyState()
  })

  it('当前 scope 无脏标记：直接放行，不弹确认框', async () => {
    await expect(confirmDiscardDirty('/points')).resolves.toBe(true)
    expect(confirmMock).not.toHaveBeenCalled()
  })

  it('有脏标记且用户确认：放行并清空该 scope', async () => {
    registerDirtyGuard('/points').mark()
    confirmMock.mockResolvedValueOnce('confirm')

    await expect(confirmDiscardDirty('/points')).resolves.toBe(true)
    expect(isScopeDirty('/points')).toBe(false)
  })

  it('有脏标记但用户取消：留在本页，且脏标记原样保留', async () => {
    registerDirtyGuard('/points').mark()
    confirmMock.mockRejectedValueOnce('cancel')

    await expect(confirmDiscardDirty('/points')).resolves.toBe(false)
    expect(isScopeDirty('/points')).toBe(true)
    expect(isAnyDirty()).toBe(true)
  })

  it('确认只清当前 scope，其他 tab 的未保存数据保留（Bug-W6）', async () => {
    registerDirtyGuard('/points').mark()
    registerDirtyGuard('/devices').mark()
    confirmMock.mockResolvedValueOnce('confirm')

    await confirmDiscardDirty('/points')

    expect(isScopeDirty('/points')).toBe(false)
    expect(isScopeDirty('/devices')).toBe(true)
  })

  it('确认框文案与按钮说明「离开会丢失」，且不自动聚焦确认按钮', async () => {
    registerDirtyGuard('/points').mark()
    confirmMock.mockResolvedValueOnce('confirm')

    await confirmDiscardDirty('/points')

    expect(confirmMock).toHaveBeenCalledWith(
      '当前有未保存的更改，离开后将丢失。',
      '未保存的更改',
      expect.objectContaining({
        confirmButtonText: '放弃更改并离开',
        cancelButtonText: '留在本页',
        // 焦点落在确认按钮上时，顺手回车就会把改动丢掉
        autofocus: false,
      }),
    )
  })

  it('存在脏标记时才挂 beforeunload，全部清除后摘除', () => {
    const addSpy = vi.spyOn(window, 'addEventListener')
    const removeSpy = vi.spyOn(window, 'removeEventListener')

    const guard = registerDirtyGuard('/points')
    guard.mark()
    const added = addSpy.mock.calls.find(([type]) => type === 'beforeunload')
    expect(added).toBeTruthy()

    guard.reset()
    const removed = removeSpy.mock.calls.find(([type]) => type === 'beforeunload')
    expect(removed).toBeTruthy()
    // 摘除的必须是挂上去的同一个 handler，否则监听器会泄漏
    expect(removed?.[1]).toBe(added?.[1])

    addSpy.mockRestore()
    removeSpy.mockRestore()
  })

  it('unregister 后该守卫的脏标记不再计入', () => {
    const guard = registerDirtyGuard('/points')
    guard.mark()
    expect(isScopeDirty('/points')).toBe(true)

    guard.unregister()
    expect(isScopeDirty('/points')).toBe(false)
    expect(isAnyDirty()).toBe(false)
  })

  it('同一 scope 内多个守卫互不影响：一个 reset 不影响另一个', async () => {
    const a = registerDirtyGuard('/points')
    const b = registerDirtyGuard('/points')
    a.mark()
    b.mark()
    a.reset()

    expect(isScopeDirty('/points')).toBe(true)

    confirmMock.mockResolvedValueOnce('confirm')
    await confirmDiscardDirty('/points')
    expect(isScopeDirty('/points')).toBe(false)
  })

  it('无 scope 的旧 API 归入空串桶，与其他 scope 隔离', () => {
    registerDirtyGuard().mark()

    expect(isAnyDirty()).toBe(true)
    expect(isScopeDirty('/points')).toBe(false)
  })
})
