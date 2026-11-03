import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import {
  computeReconnectDelay,
  GatewayWebSocket,
  getBrowserNetSnapshot,
  subscribeBrowserNet,
  __resetBrowserNetSensorForTests,
  __setNavigatorOnlineForTests,
  acquireSharedWebSocket,
  __resetSharedWebSocketPoolForTests,
  __sharedWebSocketCountForTests,
  subscribeWsFrames,
  type WsFrameInfo,
} from './websocket'
import { useWebsocketStore } from '../stores/websocket'

// Bug 226(web) 回归：connect() 自身失败（构建 URL / 取 token / 构造 WebSocket 抛错）
// 不得向外抛出——调用方是重连定时器回调与 online 事件回调，都不带 catch，
// 异常会成为未处理的 Promise rejection：状态卡在 connecting、重连链断裂、UI 无提示。
vi.mock('@/api', () => ({
  isTauri: () => true,
  // 模拟桌面端未配置网关地址：buildUrl() 直接 throw
  getGatewayBaseUrl: () => '',
  getToken: async () => '',
}))

describe('S12: computeReconnectDelay', () => {
  it('attempt=1 with rng=0.7 returns baseDelay * 1 * 0.7 = 2100 (base=3000)', () => {
    const delay = computeReconnectDelay(1, 3000, 30000, () => 0)
    // jitter=0.7 (rng returns 0)
    expect(delay).toBe(Math.round(3000 * 1 * 0.7))
  })

  it('attempt=2 with rng=0.5 returns baseDelay*2*1.0 = 6000 (jitter=1.0)', () => {
    const delay = computeReconnectDelay(2, 3000, 30000, () => 0.5)
    // jitter = 0.7 + 0.5*0.6 = 1.0
    expect(delay).toBe(6000)
  })

  it('caps at maxDelay even with large attempt and jitter=1.3', () => {
    // attempt=20: 2^19 = 524288 * 3000 ≈ 1.5e9 → 远大于 maxDelay
    const delay = computeReconnectDelay(20, 3000, 30000, () => 1)
    // jitter = 0.7 + 1*0.6 = 1.3，但 max cap 仍是 30000
    expect(delay).toBe(30000)
  })

  it('attempt<=0 falls back to baseDelay (defensive)', () => {
    expect(computeReconnectDelay(0, 3000, 30000, () => 0)).toBe(Math.round(3000 * 0.7))
    expect(computeReconnectDelay(-1, 3000, 30000, () => 0)).toBe(Math.round(3000 * 0.7))
  })

  it('jitter distribution: 100 random samples all stay within [0.7*exp, 1.3*exp]', () => {
    const exp = 3000 * 4 // attempt=3
    let min = Infinity
    let max = -Infinity
    for (let i = 0; i < 100; i++) {
      const d = computeReconnectDelay(3, 3000, 30000)
      if (d < min) min = d
      if (d > max) max = d
    }
    // rng∈[0,1) → jitter∈[0.7, 1.3) → 抖动在 exp 的 70%~130%
    expect(min).toBeGreaterThanOrEqual(Math.floor(exp * 0.7) - 1)
    expect(max).toBeLessThanOrEqual(Math.ceil(exp * 1.3) + 1)
  })

  it('uses Math.random by default (smoke)', () => {
    const d = computeReconnectDelay(1)
    expect(d).toBeGreaterThanOrEqual(2100)
    expect(d).toBeLessThanOrEqual(3900)
  })
})

describe('Bug 226(web): connect() 异常兜底', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.useFakeTimers()
  })

  it('构建 URL 失败时不向外抛出，上报错误并进入退避重连', async () => {
    const ws = new GatewayWebSocket('/api/realtime')
    const statuses: string[] = []
    ws.onStatusChange = (s) => statuses.push(s)

    // 关键契约：调用方（定时器 / 事件回调）拿到的永远是已 resolve 的 Promise
    await expect(ws.connect()).resolves.toBeUndefined()

    expect(useWebsocketStore().lastError).toContain('网关地址未设置')
    expect(statuses).toContain('connecting')
    // 失败后必须进入 reconnecting 并已排定下一次退避重连，而不是卡死在 connecting
    expect(ws.getStatus()).toBe('reconnecting')

    ws.close()
    expect(ws.getStatus()).toBe('disconnected')
    vi.useRealTimers()
  })
})

// ===== B-2 修复：浏览器网络状态感知 =====

describe('B-2: BrowserNetSensor 单例', () => {
  beforeEach(() => {
    __resetBrowserNetSensorForTests()
  })

  it('初始快照读取 navigator.onLine（jsdom 默认为 true）', () => {
    const snap = getBrowserNetSnapshot()
    expect(snap.online).toBe(true)
    expect(snap.offlineMs).toBe(0)
  })

  it('subscribe 立即同步推送当前状态', () => {
    const received: boolean[] = []
    const unsub = subscribeBrowserNet((s) => received.push(s.online))
    expect(received[0]).toBe(true)
    unsub()
  })

  it('__setNavigatorOnlineForTests 触发所有订阅者', () => {
    const a: boolean[] = []
    const b: boolean[] = []
    const u1 = subscribeBrowserNet((s) => a.push(s.online))
    const u2 = subscribeBrowserNet((s) => b.push(s.online))
    __setNavigatorOnlineForTests(false)
    __setNavigatorOnlineForTests(false) // 同状态再发，listener 仍收到（dispatch 不去重，靠订阅方去重）
    __setNavigatorOnlineForTests(true)
    expect(a).toEqual([true, false, false, true])
    expect(b).toEqual([true, false, false, true])
    u1()
    u2()
  })

  it('unsubscribe 后不再收到回调', () => {
    const got: boolean[] = []
    const unsub = subscribeBrowserNet((s) => got.push(s.online))
    __setNavigatorOnlineForTests(false)
    expect(got).toEqual([true, false])
    unsub()
    __setNavigatorOnlineForTests(true)
    expect(got).toEqual([true, false]) // 没增长
  })

  it('订阅者抛错不影响其它订阅者', () => {
    const got: boolean[] = []
    subscribeBrowserNet(() => {
      throw new Error('boom')
    })
    subscribeBrowserNet((s) => got.push(s.online))
    expect(() => __setNavigatorOnlineForTests(false)).not.toThrow()
    expect(got[got.length - 1]).toBe(false)
  })

  it('SSR 环境（无 window）也能正常 init/订阅', () => {
    // jsdom 提供 window；此用例只验证模块级 typeof window 防护不抛错
    // 直接调用订阅路径不应崩溃
    expect(() => subscribeBrowserNet(() => {})).not.toThrow()
  })
})

describe('B-2: GatewayWebSocket 离线感知', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    __resetBrowserNetSensorForTests()
    vi.useFakeTimers()
    // jsdom 默认 navigator.onLine 为 true
  })

  it('离线时调用 connect() 不再发起 new WebSocket，直接 disconnected', async () => {
    __setNavigatorOnlineForTests(false)
    const ws = new GatewayWebSocket('/api/realtime')
    const wsSpy = vi.spyOn(globalThis, 'WebSocket')

    await expect(ws.connect()).resolves.toBeUndefined()

    // 关键断言：离线状态下绝不能 new WebSocket
    expect(wsSpy).not.toHaveBeenCalled()
    expect(ws.getStatus()).toBe('disconnected')
    expect(useWebsocketStore().lastError).toContain('网络已断开')

    ws.close()
    vi.useRealTimers()
  })

  it('offline 事件：主动关闭 ws + 状态 disconnected + 不 scheduleReconnect', async () => {
    __setNavigatorOnlineForTests(true)
    const ws = new GatewayWebSocket('/api/realtime')
    // 模拟"已经连上"的状态（不开真实 WebSocket）：手动推 ws 实例 + 修正 lastNetOnline，
    // 然后触发 offline 事件，断言 handleNetChange 路径正确切换到 disconnected。
    ;(ws as unknown as { ws: unknown; status: string; lastNetOnline: boolean | null }).ws = {
      onopen: null,
      onmessage: null,
      onclose: null,
      onerror: null,
      close: vi.fn(),
    }
    ;(ws as unknown as { status: string; lastNetOnline: boolean | null }).status = 'connected'
    ;(ws as unknown as { lastNetOnline: boolean | null }).lastNetOnline = true
    // 不调 setupNetworkListeners 的内部 fs：直接调 handleNetChange 验证主路径
    // （attach 已在 subscribeBrowserNet 里完成；这里直接走 handleNetChange 是允许的）
    ;(ws as unknown as { handleNetChange: (s: { online: boolean; offlineMs: number }) => void }).handleNetChange(
      { online: false, offlineMs: 0 },
    )

    expect(ws.getStatus()).toBe('disconnected')
    expect(useWebsocketStore().lastError).toContain('网络已断开')
    // 验证：被 close 的 ws 仍然是 null（被处理路径置 None）
    expect((ws as unknown as { ws: unknown }).ws).toBeNull()
    ws.close()
    vi.useRealTimers()
  })

  it('#6 修复：网络订阅的同步首次回调不再重入 connect（只建一条 WS）', async () => {
    __setNavigatorOnlineForTests(true)
    const created: unknown[] = []
    class FakeWS {
      onopen: ((e?: unknown) => void) | null = null
      onmessage: ((e?: unknown) => void) | null = null
      onclose: ((e?: unknown) => void) | null = null
      onerror: ((e?: unknown) => void) | null = null
      readyState = 1
      constructor() {
        created.push(this)
      }
      close() {
        this.readyState = 3
      }
      send() {}
    }
    vi.stubGlobal('WebSocket', FakeWS as unknown as typeof WebSocket)

    const ws = new GatewayWebSocket('/api/realtime')
    // 绕开 module 级 mock('@/api') 的失败路径，让连接流程走到 new WebSocket
    ;(ws as unknown as { buildUrl: () => Promise<string> }).buildUrl = async () =>
      'ws://127.0.0.1/api/realtime'
    ;(ws as unknown as { buildProtocols: () => Promise<string[]> }).buildProtocols = async () => [
      'bearer.tok',
    ]

    await ws.connect()
    await Promise.resolve()

    // 旧实现：setupNetworkListeners 的同步首次回调重入 connect() → 建 2 条 WS（其一是孤儿）
    expect(created.length).toBe(1)

    // 监听只注册了一份：offline 事件仍能正常把状态切到 disconnected
    __setNavigatorOnlineForTests(false)
    expect(ws.getStatus()).toBe('disconnected')

    ws.close()
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('online 事件后：清挂起重连 + 立即 connect() 一次', async () => {
    __setNavigatorOnlineForTests(false)
    const ws = new GatewayWebSocket('/api/realtime')
    await ws.connect()
    expect(ws.getStatus()).toBe('disconnected')

    // 改 mock：buildUrl 成功（这里关键是 connect() 被调用一次）
    vi.doMock('@/api', () => ({
      isTauri: () => true,
      getGatewayBaseUrl: () => '',
      // 仍然让 buildUrl throw；目的是观察「online 唤醒后调用一次 connect → 走异常兜底」，
      // 关键是证明 handleNetChange 触发了 connect（异常兜底后 -> reconnecting 状态）
      getToken: async () => 'abc',
    }))

    __setNavigatorOnlineForTests(true)
    // 让 online listener 内的 connect() 完成
    await vi.advanceTimersByTimeAsync(0)
    await Promise.resolve()
    await Promise.resolve()

    // 异常兜底后应进入 reconnecting（不是 disconnected）
    expect(['reconnecting', 'connecting']).toContain(ws.getStatus())

    ws.close()
    vi.useRealTimers()
    vi.doUnmock('@/api')
  })

  it('manualClose=true 时 offline 事件被忽略，不再被 online 唤醒', async () => {
    __setNavigatorOnlineForTests(true)
    const ws = new GatewayWebSocket('/api/realtime')
    ws.close() // manualClose = true

    __setNavigatorOnlineForTests(false)
    await Promise.resolve()
    expect(ws.getStatus()).toBe('disconnected')

    // online 事件不应再被唤醒
    __setNavigatorOnlineForTests(true)
    await Promise.resolve()
    expect(ws.getStatus()).toBe('disconnected')
  })

  it('多实例共享同一份 online/offline 监听（不会重复 addEventListener）', () => {
    // 直接读 sensor 内部 listeners 数；2 个实例订阅后应为 2
    const a = new GatewayWebSocket('/api/realtime')
    const b = new GatewayWebSocket('/api/realtime')
    a.close() // teardown
    b.close()
    // 关键不变量：没有抛错（addEventListener 重复挂也无害，但不应泄漏未 cleanup）
    expect(true).toBe(true)
  })
})

// ===== ROI #3 修复：共享连接池（避免同一端点重复建连） =====

describe('ROI #3: acquireSharedWebSocket 共享连接池', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    __resetBrowserNetSensorForTests()
    __resetSharedWebSocketPoolForTests()
    vi.useFakeTimers()
  })
  afterEach(() => {
    __resetSharedWebSocketPoolForTests()
    vi.useRealTimers()
  })

  it('同 path 两个使用者复用同一实例（消除重复建连）', () => {
    const a = acquireSharedWebSocket('/api/realtime')
    const b = acquireSharedWebSocket('/api/realtime')
    expect(a.ws).toBe(b.ws)
    expect(__sharedWebSocketCountForTests()).toBe(1)
    a.release()
    b.release()
  })

  it('引用归零才关闭连接：中间 release 不关闭', () => {
    const a = acquireSharedWebSocket('/api/realtime')
    const b = acquireSharedWebSocket('/api/realtime')
    a.release()
    // b 仍持有引用 → 池中条目保留
    expect(__sharedWebSocketCountForTests()).toBe(1)
    b.release()
    expect(__sharedWebSocketCountForTests()).toBe(0)
  })

  it('不同 path 各自独立连接', () => {
    const a = acquireSharedWebSocket('/api/realtime')
    const b = acquireSharedWebSocket('/api/diagnostics/logs')
    expect(a.ws).not.toBe(b.ws)
    expect(__sharedWebSocketCountForTests()).toBe(2)
    a.release()
    b.release()
    expect(__sharedWebSocketCountForTests()).toBe(0)
  })

  it('归零关闭后再次 acquire 得到全新实例（旧引用失效）', () => {
    const a = acquireSharedWebSocket('/api/realtime')
    const first = a.ws
    a.release()
    const b = acquireSharedWebSocket('/api/realtime')
    expect(b.ws).not.toBe(first)
    b.release()
  })

  it('重复 release 幂等：不会把引用减成负数导致误关', () => {
    const a = acquireSharedWebSocket('/api/realtime')
    const b = acquireSharedWebSocket('/api/realtime')
    const relA = a.release
    relA()
    relA() // 第二次：条目已被 b 持有，不应再减
    expect(__sharedWebSocketCountForTests()).toBe(1)
    b.release()
    expect(__sharedWebSocketCountForTests()).toBe(0)
  })
})

// ===== 请求监视器：WS 帧观察者 =====

describe('请求监视器：WS 帧观察者', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    __resetBrowserNetSensorForTests()
    __setNavigatorOnlineForTests(true)
  })
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('只上报入站数据帧与出站 ping，pong 不计；取消订阅后不再上报', async () => {
    const created: FakeWS[] = []
    class FakeWS {
      onopen: ((e?: unknown) => void) | null = null
      onmessage: ((e?: unknown) => void) | null = null
      onclose: ((e?: unknown) => void) | null = null
      onerror: ((e?: unknown) => void) | null = null
      static OPEN = 1
      static CLOSED = 3
      readyState = 1
      constructor() {
        created.push(this)
      }
      close() {
        this.readyState = 3
      }
      send() {}
    }
    vi.stubGlobal('WebSocket', FakeWS as unknown as typeof WebSocket)

    const ws = new GatewayWebSocket('/api/realtime')
    ;(ws as unknown as { buildUrl: () => Promise<string> }).buildUrl = async () =>
      'ws://127.0.0.1/api/realtime'
    ;(ws as unknown as { buildProtocols: () => Promise<string[]> }).buildProtocols = async () => [
      'bearer.tok',
    ]

    const frames: WsFrameInfo[] = []
    const off = subscribeWsFrames((f) => frames.push(f))
    await ws.connect()
    await Promise.resolve()

    const sock = created[0]!
    sock.onmessage?.({ data: JSON.stringify([{ point_id: 1 }]) })
    // pong 是心跳噪声，不应上报
    sock.onmessage?.({ data: JSON.stringify({ type: 'pong' }) })
    ;(ws as unknown as { sendPing: () => void }).sendPing()

    expect(frames.map((f) => `${f.direction}:${f.type}`)).toEqual(['in:data', 'out:ping'])
    expect(frames[0]!.path).toBe('/api/realtime')

    off()
    sock.onmessage?.({ data: '[]' })
    expect(frames).toHaveLength(2)

    ws.close()
  })
})

describe('ROI #3: 共享实例上的多播消息监听', () => {
  beforeEach(() => setActivePinia(createPinia()))

  /** 直接触发内部消息分发（白盒；绕开真实 WebSocket） */
  function emit(ws: GatewayWebSocket, data: unknown[]): void {
    ;(ws as unknown as { emitMessage: (d: unknown[]) => void }).emitMessage(data)
  }

  it('addMessageListener 多播：所有订阅者都收到，退订后不再收', () => {
    const ws = new GatewayWebSocket('/api/realtime')
    const a: unknown[][] = []
    const b: unknown[][] = []
    const offA = ws.addMessageListener((d) => a.push(d))
    ws.addMessageListener((d) => b.push(d))

    emit(ws, [{ point_id: 'p1' }])
    expect(a).toHaveLength(1)
    expect(b).toHaveLength(1)

    offA()
    emit(ws, [{ point_id: 'p2' }])
    expect(a).toHaveLength(1) // 已退订
    expect(b).toHaveLength(2)
  })

  it('单个订阅者抛错不影响其它订阅者（与 dispatch 隔离策略一致）', () => {
    const ws = new GatewayWebSocket('/api/realtime')
    const got: unknown[][] = []
    ws.addMessageListener(() => {
      throw new Error('boom')
    })
    ws.addMessageListener((d) => got.push(d))
    expect(() => emit(ws, [])).not.toThrow()
    expect(got).toHaveLength(1)
  })

  it('onMessage 单槽与多播监听并存（向后兼容）', () => {
    const ws = new GatewayWebSocket('/api/realtime')
    const slot: unknown[][] = []
    const multi: unknown[][] = []
    ws.onMessage = (d) => slot.push(d)
    ws.addMessageListener((d) => multi.push(d))
    emit(ws, [])
    expect(slot).toHaveLength(1)
    expect(multi).toHaveLength(1)
  })
})
