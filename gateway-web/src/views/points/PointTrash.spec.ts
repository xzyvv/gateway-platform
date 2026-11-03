import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import type { PointTrashEntry } from '@/api/points'
import PointsTrash from './PointTrash.vue'
import { formatDateTime } from '@/utils/datetime'

const mockListPointTrash = vi.fn()
const mockRestorePoint = vi.fn()
const mockPurgeTrashPoint = vi.fn()
const mockPurgeAllTrash = vi.fn()
const mockConfirmAction = vi.fn()

vi.mock('@/api/points', () => ({
  listPointTrash: () => mockListPointTrash(),
  restorePoint: (id: string) => mockRestorePoint(id),
  purgeTrashPoint: (id: string) => mockPurgeTrashPoint(id),
  purgeAllTrash: () => mockPurgeAllTrash(),
}))

vi.mock('@/utils/confirm', () => ({
  confirmAction: (opts: { message: string }) => mockConfirmAction(opts),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

const ENTRY = {
  point: {
    id: 'p1',
    name: '进水温度',
    device_id: 'dev1',
    bus: 'bus1',
    protocol: 'modbus',
    address: '40001',
  },
  deleted_at_ms: 1700000000000,
} as unknown as PointTrashEntry

/** 挂载并等待 onMounted 的列表请求完成 */
async function mountTrash() {
  const wrapper = mount(PointsTrash)
  await flushPromises()
  await nextTick()
  return wrapper
}

function findButton(wrapper: Awaited<ReturnType<typeof mountTrash>>, text: string) {
  return wrapper.findAll('button').find((b) => b.text().includes(text))
}

describe('PointsTrash.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockListPointTrash.mockResolvedValue([ENTRY])
    mockRestorePoint.mockResolvedValue(null)
    mockPurgeTrashPoint.mockResolvedValue(null)
    mockPurgeAllTrash.mockResolvedValue(null)
    mockConfirmAction.mockResolvedValue(true)
  })

  it('加载后渲染点位 ID、名称与删除时间', async () => {
    const wrapper = await mountTrash()
    const text = wrapper.text()
    expect(text).toContain('p1')
    expect(text).toContain('进水温度')
    expect(text).toContain(formatDateTime(ENTRY.deleted_at_ms))
  })

  it('点击恢复调用 restorePoint 并重新加载列表', async () => {
    const wrapper = await mountTrash()
    const btn = findButton(wrapper, '恢复')
    expect(btn).toBeTruthy()

    await btn!.trigger('click')
    await flushPromises()
    await nextTick()

    expect(mockRestorePoint).toHaveBeenCalledWith('p1')
    expect(mockListPointTrash).toHaveBeenCalledTimes(2)
  })

  it('彻底删除取消确认时不调用接口', async () => {
    mockConfirmAction.mockResolvedValue(false)
    const wrapper = await mountTrash()

    await findButton(wrapper, '彻底删除')!.trigger('click')
    await flushPromises()

    expect(mockConfirmAction).toHaveBeenCalled()
    expect(mockPurgeTrashPoint).not.toHaveBeenCalled()
  })

  it('确认后彻底删除并重新加载列表', async () => {
    const wrapper = await mountTrash()

    await findButton(wrapper, '彻底删除')!.trigger('click')
    await flushPromises()
    await nextTick()

    expect(String(mockConfirmAction.mock.calls[0]?.[0]?.message)).toContain('不可恢复')
    expect(mockPurgeTrashPoint).toHaveBeenCalledWith('p1')
    expect(mockListPointTrash).toHaveBeenCalledTimes(2)
  })

  it('回收站为空时展示空态文案', async () => {
    mockListPointTrash.mockResolvedValue([])
    const wrapper = await mountTrash()

    expect(wrapper.text()).toContain('暂无已删除点位，删除的点位会暂存 30 天后自动清理')
  })
})
