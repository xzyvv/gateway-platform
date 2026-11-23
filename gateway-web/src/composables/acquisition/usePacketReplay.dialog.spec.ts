import { describe, it, expect, vi, beforeEach } from 'vitest'
import { defineComponent, h, ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

/**
 * 原生 prompt()/confirm() → Element Plus 弹窗的回归钉子（13 号报告 §2.1）。
 *
 * 关键契约：**用户取消不得触发写操作**。原生 prompt 取消返回 null、confirm 返回 false；
 * 换成 ElMessageBox.prompt（取消时 reject）与 confirmDelete（取消返回 false）后，
 * 两条取消路径都必须保持"不保存 / 不删除"，否则用户点个取消就误删会话。
 */
const promptMock = vi.hoisted(() => vi.fn())
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessageBox: { ...actual.ElMessageBox, prompt: promptMock } }
})

const confirmDeleteMock = vi.hoisted(() => vi.fn())
vi.mock('@/utils/confirm', () => ({ confirmDelete: confirmDeleteMock }))

const apiMock = vi.hoisted(() => ({
  listSessions: vi.fn(),
  getSession: vi.fn(),
  saveSession: vi.fn(),
  deleteSession: vi.fn(),
  executeReplayBlocking: vi.fn(),
  executeReplayStream: vi.fn(),
  canStreamReplay: vi.fn(),
}))
vi.mock('@/api/protocolTools', () => ({
  listSessions: apiMock.listSessions,
  getSession: apiMock.getSession,
  saveSession: apiMock.saveSession,
  deleteSession: apiMock.deleteSession,
  executeReplayBlocking: apiMock.executeReplayBlocking,
  executeReplayStream: apiMock.executeReplayStream,
  canStreamReplay: apiMock.canStreamReplay,
}))
vi.mock('@/composables/core/useErrorDisplay', () => ({
  useErrorDisplay: () => ({ message: vi.fn(), showToast: vi.fn() }),
}))
vi.mock('@/composables/acquisition/useDebugTargets', () => ({
  useDebugTargets: () => ({
    busOptions: ref([]),
    loading: ref(false),
    selectedBus: ref('/dev/ttyUSB0'),
  }),
}))
vi.mock('@/composables/core/useRequestGuard', () => ({
  useRequestGuard: () => ({ next: () => 1, isCurrent: () => true }),
}))

import { usePacketReplay } from './usePacketReplay'
import type { ReplaySession } from '@/api/protocolTools'

const SESSION: ReplaySession = {
  id: 'u1',
  name: '会话A',
  deviceName: '温控器',
  durationMs: 1000,
  packetCount: 1,
  createdAt: 0,
  packets: [{ id: 'p1', time: 0, direction: 'request', hex: '01 03 00 01', parsed: '' }],
}

/** 在真实组件实例里跑 composable：onMounted 会拉取会话列表并选中第一条 */
function withSetup() {
  let result!: ReturnType<typeof usePacketReplay>
  const wrapper = mount(
    defineComponent({
      setup() {
        result = usePacketReplay()
        return () => h('div')
      },
    }),
  )
  return { result, wrapper }
}

describe('usePacketReplay — 弹窗改成 Element Plus 后取消不写数据', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    apiMock.listSessions.mockResolvedValue([SESSION])
    apiMock.getSession.mockResolvedValue(SESSION)
    apiMock.saveSession.mockImplementation(async (s: ReplaySession) => s)
    apiMock.deleteSession.mockResolvedValue(undefined)
  })

  it('另存会话：确认后按输入名保存，默认值带「（副本）」', async () => {
    promptMock.mockResolvedValue({ value: '会话A 副本' })
    const { result, wrapper } = withSetup()
    await flushPromises() // onMounted → select → session.value 就绪

    await result.saveAsNew()

    expect(promptMock).toHaveBeenCalledWith('新会话名称：', '另存回放会话', {
      inputValue: '会话A（副本）',
    })
    expect(apiMock.saveSession).toHaveBeenCalledWith(
      expect.objectContaining({ name: '会话A 副本', id: expect.stringMatching(/^u_/) }),
    )
    wrapper.unmount()
  })

  it('另存会话：取消（prompt reject）不落库', async () => {
    promptMock.mockRejectedValue('cancel')
    const { result, wrapper } = withSetup()
    await flushPromises()

    await result.saveAsNew()

    expect(apiMock.saveSession).not.toHaveBeenCalled()
    wrapper.unmount()
  })

  it('删除会话：确认后调用 deleteSession', async () => {
    confirmDeleteMock.mockResolvedValue(true)
    const { result, wrapper } = withSetup()
    await flushPromises()

    await result.doDelete('u1')

    expect(confirmDeleteMock).toHaveBeenCalledWith('会话A')
    expect(apiMock.deleteSession).toHaveBeenCalledWith('u1')
    wrapper.unmount()
  })

  it('删除会话：取消不调用 deleteSession', async () => {
    confirmDeleteMock.mockResolvedValue(false)
    const { result, wrapper } = withSetup()
    await flushPromises()

    await result.doDelete('u1')

    expect(confirmDeleteMock).toHaveBeenCalledWith('会话A')
    expect(apiMock.deleteSession).not.toHaveBeenCalled()
    wrapper.unmount()
  })
})
