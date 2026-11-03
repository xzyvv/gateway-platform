import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'

vi.mock('@/api/protocolTools', () => ({
  listSessions: vi.fn(),
  getSession: vi.fn(),
  executeReplayBlocking: vi.fn(),
  executeReplayStream: vi.fn(),
  canStreamReplay: vi.fn(),
}))
vi.mock('@/composables/acquisition/useDebugTargets', () => ({
  useDebugTargets: () => ({
    busOptions: ref([{ label: '调试总线', value: '/dev/ttyUSB0' }]),
    loading: ref(false),
    selectedBus: ref('/dev/ttyUSB0'),
  }),
}))

import PacketReplay from './PacketReplay.vue'
import {
  listSessions,
  getSession,
  executeReplayBlocking,
  executeReplayStream,
  canStreamReplay,
  type ReplaySession,
  type ReplayExecuteResult,
} from '@/api/protocolTools'

/**
 * P0-1b 契约 §6/§8 #3：回放模式选择的回归钉子——
 * 环境支持流式时走 SSE（带 AbortSignal，可中途停止）；
 * 旧浏览器 / Tauri（canStreamReplay=false）回落 v1 阻塞模式，不建流。
 */
const session: ReplaySession = {
  id: 'rs_1',
  name: '会话A',
  deviceName: '设备甲',
  durationMs: 1200,
  packetCount: 1,
  createdAt: 0,
  packets: [
    { id: 'p1', time: 0, direction: 'request', hex: '01 03 00 00 00 02', parsed: '读保持' },
    { id: 'p2', time: 100, direction: 'response', hex: '01 03 04 00 2A 00 00', parsed: '回' },
  ],
}

const frame: ReplayExecuteResult['results'][number] = {
  packetId: 'p1',
  requestHex: '01 03 00 00 00 02',
  expectedResponseHex: '01 03 04 00 2A 00 00',
  actualResponseHex: '01 03 04 00 2A 00 00',
  status: 'match',
  latencyMs: 42,
}

const summary: ReplayExecuteResult = {
  sessionId: 'rs_1',
  bus: '/dev/ttyUSB0',
  startedAtMs: 1000,
  finishedAtMs: 1600,
  total: 1,
  match: 1,
  mismatch: 0,
  failed: 0,
  results: [frame],
}

async function mountAndClickReplay() {
  const wrapper = mount(PacketReplay)
  await flushPromises() // onMounted: listSessions → select → getSession
  const btn = wrapper.findAll('button').find((b) => b.text() === '回放')
  expect(btn).toBeTruthy()
  await btn!.trigger('click')
  await flushPromises()
  return wrapper
}

describe('PacketReplay 回放模式选择（p0-1b §6/§8）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(listSessions).mockResolvedValue([session])
    vi.mocked(getSession).mockResolvedValue(session)
  })

  it('环境支持流式：走 executeReplayStream（带 AbortSignal），summary 落地渲染', async () => {
    vi.mocked(canStreamReplay).mockReturnValue(true)
    vi.mocked(executeReplayStream).mockImplementation(async (_id, _bus, _opts, handlers) => {
      handlers.onStarted?.({ sessionId: 'rs_1', bus: '/dev/ttyUSB0', total: 1, startedAtMs: 1000 })
      handlers.onFrame?.(frame)
      handlers.onSummary?.(summary)
    })

    const wrapper = await mountAndClickReplay()

    expect(executeReplayStream).toHaveBeenCalledOnce()
    const [id, bus, opts, handlers] = vi.mocked(executeReplayStream).mock.calls[0]!
    expect(id).toBe('rs_1')
    expect(bus).toBe('/dev/ttyUSB0')
    expect(opts).toEqual({ timeoutMs: 1500, intervalMs: 200, stopOnMismatch: false })
    expect(handlers.signal).toBeInstanceOf(AbortSignal)
    expect(executeReplayBlocking).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('一致 1')
  })

  it('环境不支持流式（canStreamReplay=false）：回落阻塞模式，不建流', async () => {
    vi.mocked(canStreamReplay).mockReturnValue(false)
    vi.mocked(executeReplayBlocking).mockResolvedValue(summary)

    const wrapper = await mountAndClickReplay()

    expect(executeReplayBlocking).toHaveBeenCalledWith('rs_1', '/dev/ttyUSB0', {
      timeoutMs: 1500,
      intervalMs: 200,
      stopOnMismatch: false,
    })
    expect(executeReplayStream).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('一致 1')
  })
})
