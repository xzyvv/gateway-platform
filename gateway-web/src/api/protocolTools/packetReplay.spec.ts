import { describe, it, expect, afterEach, vi } from 'vitest'
import { canStreamReplay } from './packetReplay'

/**
 * P0-1b 契约 §8 #3：流式回放的环境探测。
 * fetch + ReadableStream 齐备且非 Tauri → 流式；缺任一条件 → 调用方回落阻塞模式。
 * 这里钉住谓词逻辑本身（显式桩掉/装回全局），不依赖 jsdom 是否自带这些 API。
 */
describe('canStreamReplay 环境探测', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    delete window.__TAURI_INTERNALS__
  })

  it('fetch 与 ReadableStream 齐备（现代 Chromium）：可用', () => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('ReadableStream', class {})
    expect(canStreamReplay()).toBe(true)
  })

  it('缺 ReadableStream（旧浏览器）：不可用', () => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('ReadableStream', undefined)
    expect(canStreamReplay()).toBe(false)
  })

  it('缺 fetch：不可用', () => {
    vi.stubGlobal('fetch', undefined)
    vi.stubGlobal('ReadableStream', class {})
    expect(canStreamReplay()).toBe(false)
  })

  it('Tauri 桌面端即使 API 齐备也不可用（相对 URL 的 fetch 不指向网关）', () => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('ReadableStream', class {})
    window.__TAURI_INTERNALS__ = {}
    expect(canStreamReplay()).toBe(false)
  })
})
