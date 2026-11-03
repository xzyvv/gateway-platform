import { describe, it, expect, beforeEach, vi } from 'vitest'

// 用模块级对象模拟「当前激活网关」，测试内可随时切换
const active = { name: null as string | null }

type GatewayStorageModule = typeof import('./gatewayStorage')
let store: GatewayStorageModule

/**
 * 不能静态 import 被测模块：setup.ts 会预加载 gatewayStorage（进而加载
 * gatewayConfig），静态 import 拿到的会是真实模块（命名空间恒为空），mock 不生效。
 * 每个用例先 resetModules + doMock，再动态 import，保证被测模块取到本文件的 mock。
 */
beforeEach(async () => {
  localStorage.clear()
  active.name = null
  vi.resetModules()
  vi.doMock('@/api/gatewayConfig', () => ({
    getActiveGatewayName: () => active.name,
  }))
  store = await import('./gatewayStorage')
})

describe('gatewayStorage 多网关隔离 (#6)', () => {
  it('无激活网关时保持原始 key（Web 模式行为不变）', () => {
    expect(store.gatewayNamespace()).toBe('')
    expect(store.scopedKey('k')).toBe('k')
    store.scopedSet('k', 'v')
    expect(localStorage.getItem('k')).toBe('v')
    expect(store.scopedGet('k')).toBe('v')
  })

  it('有激活网关时按网关名隔离存储', () => {
    active.name = 'gwA'
    store.scopedSet('k', 'v')
    expect(localStorage.getItem('gw:gwA::k')).toBe('v')
    expect(localStorage.getItem('k')).toBeNull()
    expect(store.scopedGet('k')).toBe('v')
  })

  it('不同网关的同名 key 互不可见（不串台）', () => {
    active.name = 'gwA'
    store.scopedSet('k', 'A')
    active.name = 'gwB'
    store.scopedSet('k', 'B')
    expect(store.scopedGet('k')).toBe('B')
    active.name = 'gwA'
    expect(store.scopedGet('k')).toBe('A')
  })

  it('升级前的无前缀数据会迁移到当前网关命名空间', () => {
    localStorage.setItem('k', 'legacy')
    active.name = 'gwA'
    expect(store.scopedGet('k')).toBe('legacy')
    expect(localStorage.getItem('gw:gwA::k')).toBe('legacy')
  })

  it('scopedRemove 只删除当前网关的项', () => {
    active.name = 'gwA'
    store.scopedSet('k', 'A')
    active.name = 'gwB'
    store.scopedSet('k', 'B')
    active.name = 'gwA'
    store.scopedRemove('k')
    expect(store.scopedGet('k')).toBeNull()
    active.name = 'gwB'
    expect(store.scopedGet('k')).toBe('B')
  })
})
