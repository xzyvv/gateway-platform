import { describe, it, expect, vi } from 'vitest'

// DD-27：只测 isBuiltin 这个纯函数，mock 掉模块加载链上的外部依赖，避免副作用。
vi.mock('@/api/protocolTools', () => ({
  listSessions: vi.fn(),
  getSession: vi.fn(),
  executeReplayBlocking: vi.fn(),
  executeReplayStream: vi.fn(),
  canStreamReplay: vi.fn(),
  saveSession: vi.fn(),
  deleteSession: vi.fn(),
}))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ message: vi.fn() }) }))
vi.mock('@/composables/acquisition/useDebugTargets', () => ({ useDebugTargets: () => ({}) }))
vi.mock('@/composables/core/useRequestGuard', () => ({ useRequestGuard: () => ({}) }))

import { isBuiltin } from './usePacketReplay'

describe('DD-27 内置回放会话白名单与后端对齐', () => {
  it('后端内置的 s1 / s2 被识别为内置会话', () => {
    expect(isBuiltin('s1')).toBe(true)
    expect(isBuiltin('s2')).toBe(true)
  })

  it('不存在的 s3 不得被识别为内置（曾导致点开必 404）', () => {
    expect(isBuiltin('s3')).toBe(false)
  })

  it('用户自建会话 ID 不是内置', () => {
    expect(isBuiltin('my-custom-session')).toBe(false)
  })
})
