// Vitest 全局测试环境初始化
// jsdom 环境下需要 mock localStorage、URL.createObjectURL 等浏览器 API

// localStorage 已由 jsdom 提供，无需额外 mock

// Web Crypto polyfill：jsdom 默认不暴露 globalThis.crypto.subtle，
// 但 Node 22+ 内置 webcrypto，把它挂到 globalThis 上供 secureStorage 等使用。
// 走 createRequire 模式：vitest 把 .ts 编译为 CJS 后 `require` 全局可用。
// devDependencies 已含 @types/node，node:crypto 的类型可直接解析。
// Pinia 全局初始化：每个测试用例前创建全新实例并设为 active。
// axios 拦截器等组件外代码调用 useXxxStore() 时需要 active Pinia，
// 否则报错 "getActivePinia() was called but there was no active Pinia"。
import { beforeEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
// 共享 tick 是模块级调度器：每个用例前清空订阅并停表，避免上一个用例挂载的组件
// 或用例内的 fake timers 切换留下悬挂句柄（会让模块误判「已有定时器」而不重建）。
import { __resetSharedTickForTests } from '../composables/core/useSharedTick'
// usePolling 已改为模块级共享调度器（同一策略）：每个用例前清空订阅/停表/归零虚拟时钟。
import { __resetUsePollingForTests } from '../composables/core/usePolling'

beforeEach(() => {
  setActivePinia(createPinia())
  __resetSharedTickForTests()
  __resetUsePollingForTests()
})
import { webcrypto } from 'node:crypto'
if (typeof globalThis.crypto === 'undefined' || !globalThis.crypto.subtle) {
  Object.defineProperty(globalThis, 'crypto', { value: webcrypto, configurable: true })
}

// mock URL.createObjectURL / revokeObjectURL（CSV 导出测试需要）
if (typeof URL.createObjectURL !== 'function') {
  URL.createObjectURL = () => 'blob:mock-url'
  URL.revokeObjectURL = () => {}
}

// ResizeObserver / IntersectionObserver polyfill：
// jsdom 不实现这两个观察器，但 Element Plus 的 el-table（TableLayout）等组件
// 会在挂载时构造它们；缺省会抛 ReferenceError 导致含 el-table 的组件测试失败。
// 此处注册无操作 stub：观察/取消都不做，仅满足"存在该构造器"的运行时检查。
class ResizeObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
}
class IntersectionObserverStub {
  observe() {}
  unobserve() {}
  disconnect() {}
  takeRecords() { return [] }
  root = null
  rootMargin = ''
  thresholds = []
}
if (typeof globalThis.ResizeObserver === 'undefined') {
  Object.defineProperty(globalThis, 'ResizeObserver', { value: ResizeObserverStub, configurable: true, writable: true })
}
if (typeof globalThis.IntersectionObserver === 'undefined') {
  Object.defineProperty(globalThis, 'IntersectionObserver', { value: IntersectionObserverStub, configurable: true, writable: true })
}

// 清空本地存储，避免测试间串扰（前端本地操作审计已下线，只需清 localStorage）
localStorage.clear()

// 注册全局组件，供测试环境使用（与 main.ts 保持一致）
import { config } from '@vue/test-utils'
import type { Component } from 'vue'
import EmptyState from '../components/EmptyState.vue'
import SectionHeader from '../components/SectionHeader.vue'
config.global.components['EmptyState'] = EmptyState
config.global.components['SectionHeader'] = SectionHeader

// Element Plus 图标全局注册（与 main.ts 保持一致）：
// 业务模板直接用 <el-icon><Refresh /></el-icon> 等写法，依赖 main.ts 的全局注册；
// 测试挂载若不注册会刷 "Failed to resolve component" warn。
import * as ElIcons from '@element-plus/icons-vue'
for (const [name, comp] of Object.entries(ElIcons)) {
  config.global.components[name] = comp as Component
}