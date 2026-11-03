import { describe, it, expect, afterEach } from 'vitest'

import { isTauri } from './env'

describe('env（Tauri 环境检测）', () => {
  afterEach(() => {
    // 清掉用例注入的标记，避免污染其它用例
    delete window.__TAURI_INTERNALS__
  })

  // 锁定默认行为：Web 端（浏览器）没有 Tauri 运行时注入的对象 → 必须判为 false。
  // 判错会导致 Web 端误走 tauriAdapter（所有请求经 invoke 转发），功能整体失效。
  it('Web 端（无 __TAURI_INTERNALS__）判定为非 Tauri', () => {
    expect(isTauri()).toBe(false)
  })

  // 锁定 Tauri 端判定：只要注入对象存在（不校验其内容）即为 true
  it('注入 __TAURI_INTERNALS__ 后判定为 Tauri', () => {
    window.__TAURI_INTERNALS__ = {}
    expect(isTauri()).toBe(true)
  })

  // 边界：注入对象为 null / undefined / 0 等假值时仍判为 false（用的是真值判断）
  it('注入假值时仍判定为非 Tauri', () => {
    window.__TAURI_INTERNALS__ = null
    expect(isTauri()).toBe(false)

    window.__TAURI_INTERNALS__ = 0
    expect(isTauri()).toBe(false)

    window.__TAURI_INTERNALS__ = ''
    expect(isTauri()).toBe(false)
  })

  // 锁定判定时机：isTauri() 每次调用都重新读 window.__TAURI_INTERNALS__，
  // 不得在模块加载时把结果缓存下来 —— Tauri 注入发生在页面启动之后，
  // 一旦被缓存，桌面端会被误判成 Web 端（反向：删掉标记后仍判 true 则说明缓存了）。
  // 注：SSR「完全没有 window」这条分支在 jsdom 下无法构造
  // （jsdom 的 window 是不可配置属性，vi.stubGlobal 无法覆盖），故不单列用例。
  it('每次调用实时读取标记，不缓存判定结果', () => {
    expect(isTauri()).toBe(false)

    window.__TAURI_INTERNALS__ = {}
    expect(isTauri()).toBe(true)

    delete window.__TAURI_INTERNALS__
    expect(isTauri()).toBe(false)
  })
})
