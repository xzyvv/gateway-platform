import { describe, it, expect, vi, beforeEach } from 'vitest'
import { ElMessageBox } from 'element-plus'
import { confirmAction, confirmDelete } from './confirm'

vi.mock('element-plus', () => ({
  ElMessageBox: { confirm: vi.fn() },
}))

/**
 * 确认框封装是全仓唯一入口，此前没有单测。
 *
 * 核心契约：**取消不抛错、返回 false**。所有调用方都写成
 * `if (!(await confirmAction(...))) return`，这里一旦改成抛错，
 * 几百处调用会变成未捕获拒绝。
 */
describe('confirmAction', () => {
  const confirmMock = ElMessageBox.confirm as ReturnType<typeof vi.fn>

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('用户确认返回 true', async () => {
    confirmMock.mockResolvedValueOnce('confirm')
    await expect(confirmAction({ message: '确定？' })).resolves.toBe(true)
  })

  it('用户取消返回 false 且不抛错', async () => {
    confirmMock.mockRejectedValueOnce('cancel')
    await expect(confirmAction({ message: '确定？' })).resolves.toBe(false)
  })

  it('关闭对话框（返回 close）与取消同样处理', async () => {
    confirmMock.mockRejectedValueOnce('close')
    await expect(confirmAction({ message: '确定？' })).resolves.toBe(false)
  })

  it('未传的选项走默认值：标题「操作确认」、警告图标、确定/取消', async () => {
    confirmMock.mockResolvedValueOnce('confirm')
    await confirmAction({ message: '确定？' })

    expect(confirmMock).toHaveBeenCalledWith('确定？', '操作确认', {
      type: 'warning',
      confirmButtonText: '确定',
      cancelButtonText: '取消',
      autofocus: true,
    })
  })

  it('自定义标题/图标/按钮文案原样透传', async () => {
    confirmMock.mockResolvedValueOnce('confirm')
    await confirmAction({
      message: '将覆盖当前配置',
      title: '套用模板',
      type: 'info',
      confirmText: '覆盖并应用',
      cancelText: '再想想',
    })

    expect(confirmMock).toHaveBeenCalledWith('将覆盖当前配置', '套用模板', {
      type: 'info',
      confirmButtonText: '覆盖并应用',
      cancelButtonText: '再想想',
      autofocus: true,
    })
  })

  it('autofocus: false 必须透传（放弃未保存修改时不能聚焦确认按钮）', async () => {
    confirmMock.mockResolvedValueOnce('confirm')
    await confirmAction({ message: '改动会丢失', autofocus: false })

    expect(confirmMock).toHaveBeenCalledWith(
      '改动会丢失',
      '操作确认',
      expect.objectContaining({ autofocus: false }),
    )
  })
})

describe('confirmDelete', () => {
  const confirmMock = ElMessageBox.confirm as ReturnType<typeof vi.fn>

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('默认文案声明「不可恢复」，按钮为「删除」', async () => {
    confirmMock.mockResolvedValueOnce('confirm')
    await confirmDelete('点位 A')

    expect(confirmMock).toHaveBeenCalledWith(
      '确定删除「点位 A」？删除后不可恢复',
      '删除确认',
      expect.objectContaining({ confirmButtonText: '删除', type: 'warning' }),
    )
  })

  it('传 undoSeconds 时改口为可撤销，与实际行为一致', async () => {
    confirmMock.mockResolvedValueOnce('confirm')
    await confirmDelete('点位 A', { undoSeconds: 10 })

    expect(confirmMock).toHaveBeenCalledWith(
      '确定删除「点位 A」？删除后 10 秒内可在通知中撤销',
      '删除确认',
      expect.anything(),
    )
  })

  it('自定义按钮与标题（禁用类操作复用删除确认）', async () => {
    confirmMock.mockResolvedValueOnce('confirm')
    await confirmDelete('通道 1', { title: '停用通道', confirmText: '停用', type: 'error' })

    expect(confirmMock).toHaveBeenCalledWith(
      expect.stringContaining('确定删除「通道 1」？'),
      '停用通道',
      expect.objectContaining({ confirmButtonText: '停用', type: 'error' }),
    )
  })
})
