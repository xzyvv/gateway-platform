import { describe, it, expect, beforeEach, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useWebsocketStore } from './websocket'

describe('useWebsocketStore', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
  })

  it('初始状态：disconnected / 0 / null / 0', () => {
    const store = useWebsocketStore()
    expect(store.status).toBe('disconnected')
    expect(store.reconnectCount).toBe(0)
    expect(store.lastError).toBeNull()
    expect(store.subscribers).toBe(0)
    expect(store.isConnected).toBe(false)
    expect(store.isReconnecting).toBe(false)
  })

  it('setStatus 切换连接态并联动 isConnected', () => {
    const store = useWebsocketStore()
    store.setStatus('connecting')
    expect(store.status).toBe('connecting')
    expect(store.isConnected).toBe(false)
    store.setStatus('connected')
    expect(store.status).toBe('connected')
    expect(store.isConnected).toBe(true)
    store.setStatus('disconnected')
    expect(store.status).toBe('disconnected')
    expect(store.isConnected).toBe(false)
  })

  it('incrementReconnect / resetReconnect 计数', () => {
    const store = useWebsocketStore()
    expect(store.reconnectCount).toBe(0)
    store.incrementReconnect()
    store.incrementReconnect()
    expect(store.reconnectCount).toBe(2)
    store.resetReconnect()
    expect(store.reconnectCount).toBe(0)
  })

  it('setLastError 设置与清除', () => {
    const store = useWebsocketStore()
    expect(store.lastError).toBeNull()
    store.setLastError('连接被拒')
    expect(store.lastError).toBe('连接被拒')
    store.setLastError(null)
    expect(store.lastError).toBeNull()
  })

  it('subscribe / unsubscribe 计数且不低于 0', () => {
    const store = useWebsocketStore()
    expect(store.subscribers).toBe(0)
    store.subscribe()
    store.subscribe()
    store.subscribe()
    expect(store.subscribers).toBe(3)
    store.unsubscribe()
    expect(store.subscribers).toBe(2)
    // 连续 unsubscribe 不会降到负数
    store.unsubscribe()
    store.unsubscribe()
    store.unsubscribe()
    expect(store.subscribers).toBe(0)
  })

  it('isReconnecting：connecting 且重连次数>0', () => {
    const store = useWebsocketStore()
    // 仅 connecting 不算重连
    store.setStatus('connecting')
    expect(store.isReconnecting).toBe(false)
    // connecting + 重连次数>0 视为重连中
    store.incrementReconnect()
    expect(store.isReconnecting).toBe(true)
    // 连接成功后归位
    store.setStatus('connected')
    store.resetReconnect()
    expect(store.isReconnecting).toBe(false)
    expect(store.isConnected).toBe(true)
  })

  it('mock timer：模拟重连定时器累计重连计数', () => {
    vi.useFakeTimers()
    try {
      const store = useWebsocketStore()
      store.setStatus('connecting')
      // 模拟每次重连失败后定时器触发 incrementReconnect
      const timer: ReturnType<typeof setInterval> = setInterval(() => store.incrementReconnect(), 3000)
      vi.advanceTimersByTime(9000) // 触发 3 次
      expect(store.reconnectCount).toBe(3)
      expect(store.isReconnecting).toBe(true)
      clearInterval(timer)
      // 重连成功归零
      store.resetReconnect()
      store.setStatus('connected')
      expect(store.reconnectCount).toBe(0)
      expect(store.isConnected).toBe(true)
    } finally {
      vi.useRealTimers()
    }
  })
})