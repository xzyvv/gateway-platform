import { describe, it, expect, beforeEach, vi } from 'vitest'
import { setActivePinia, createPinia } from 'pinia'
import { ElMessage, ElNotification } from 'element-plus'
import { useNotificationsStore } from './notifications'

vi.mock('element-plus', () => ({
  ElMessage: vi.fn(),
  ElNotification: vi.fn(),
}))

describe('notifications store — 3 秒去重', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.clearAllMocks()
    vi.restoreAllMocks()
  })

  it('首次 error 入队并弹 Notification', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000)
    const store = useNotificationsStore()

    store.error('设备离线')

    expect(store.items).toHaveLength(1)
    expect(ElNotification).toHaveBeenCalledTimes(1)
  })

  it('3 秒内相同 message 不重复弹，且队列不新增条目', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000)
    const store = useNotificationsStore()
    store.error('设备离线')
    expect(ElNotification).toHaveBeenCalledTimes(1)

    // 2 秒后同一条错误（典型场景：3s 轮询再次失败）
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 + 2000)
    store.error('设备离线')

    expect(ElNotification).toHaveBeenCalledTimes(1) // 关键：不再弹第二次
    expect(store.items).toHaveLength(1)
  })

  it('持续重复的同一条错误（跨过 3 秒）也只保留一条、只弹一次', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000)
    const store = useNotificationsStore()
    store.error('网络错误: timeout')

    // 失联期间每 2 秒一次的轮询失败：滑动窗口让同一条记录不断续期
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 + 2000)
    store.error('网络错误: timeout')
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 + 4000)
    store.error('网络错误: timeout')
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 + 6000)
    store.error('网络错误: timeout')

    expect(ElNotification).toHaveBeenCalledTimes(1) // 关键：断网期间不刷屏
    expect(store.items).toHaveLength(1)
    expect(store.items[0]!.timestamp).toBe(1_000_000 + 6000) // 时间戳跟随最近一次
  })

  it('错误停止 3 秒后再出现：新建条目并重新提醒', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000)
    const store = useNotificationsStore()
    store.error('设备离线')

    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 + 3500)
    store.error('设备离线')

    expect(ElNotification).toHaveBeenCalledTimes(2)
    expect(store.items).toHaveLength(2)
  })

  it('同一时刻不同文案的错误：全部入队、弹窗被节流为一次', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000)
    const store = useNotificationsStore()
    store.error('请求失败(502): /api/devices')
    store.error('请求失败(502): /api/points')

    expect(store.items).toHaveLength(2) // 信息不丢：通知中心仍能看到两条
    expect(ElNotification).toHaveBeenCalledTimes(1) // 但只弹一次

    // 3 秒后另一条不同错误恢复弹窗
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000 + 3001)
    store.error('请求失败(502): /api/config')
    expect(ElNotification).toHaveBeenCalledTimes(2)
  })

  it('非 error/warning 走 toast 且同样去重', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000)
    const store = useNotificationsStore()
    store.success('保存成功')
    store.success('保存成功')

    expect(ElMessage).toHaveBeenCalledTimes(1)
    expect(store.items).toHaveLength(1)
  })

  it('silent: true 只入队、不弹窗（写操作失败由调用方提示）', () => {
    vi.spyOn(Date, 'now').mockReturnValue(1_000_000)
    const store = useNotificationsStore()
    store.error('保存失败', { silent: true })

    expect(ElNotification).not.toHaveBeenCalled()
    expect(ElMessage).not.toHaveBeenCalled()
    // 通知中心仍可见，便于事后排查
    expect(store.items).toHaveLength(1)
    expect(store.items[0]!.message).toBe('保存失败')
  })
})
