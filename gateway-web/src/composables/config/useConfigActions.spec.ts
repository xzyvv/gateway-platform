/**
 * 修复(审查#B7) 回归：配置保存的「在飞」保护。
 *
 * 反证：删掉 useConfigActions.saveConfig 开头的 `if (saving.value) return` 后，
 * 连续两次 saveConfig() 会各自走完 confirmAction + postConfig + 热重载，
 * postConfig 被调用 2 次 → 本用例失败；恢复修复后通过。
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

const m = vi.hoisted(() => ({
  push: vi.fn(),
  postConfig: vi.fn(async (..._a: unknown[]) => ({ status: 'ok' })),
  confirmAction: vi.fn(async (..._a: unknown[]) => true),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: m.push }),
  // Bug-W6 (fix)：useConfigActions 用 route.path 作为 dirty guard scope，
  // 测试环境 mock 一个稳定的 path。真实组件挂载时由 Vue Router 注入实际 path。
  useRoute: () => ({ path: '/test-config', query: {} }),
}))
vi.mock('@/utils/confirm', () => ({ confirmAction: (...a: unknown[]) => m.confirmAction(...a) }))
vi.mock('@/utils/dirtyGuard', () => ({
  registerDirtyGuard: () => ({ reset: vi.fn(), mark: vi.fn(), unregister: vi.fn() }),
}))
vi.mock('@/composables/config/configValidation', () => ({ validateConfig: async () => [] }))
vi.mock('@/api/config', () => ({
  postConfig: (...a: unknown[]) => m.postConfig(...a),
  exportConfig: async () => ({}),
  reloadConfig: async () => ({ status: 'ok', notes: [] }),
  getConfig: async () => ({}),
  importConfig: async () => ({ status: 'ok' }),
}))
vi.mock('@/api', () => ({ setToken: vi.fn(async () => {}) }))
vi.mock('@/composables/uplink/useWebhook', () => ({
  useWebhook: () => ({ notify: vi.fn(async () => {}) }),
}))
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElMessageBox: { alert: vi.fn() },
  }
})

import { useConfigActions } from './useConfigActions'
import { ElMessage } from 'element-plus'

const mounted: Array<{ unmount: () => void }> = []

function harness() {
  let api: ReturnType<typeof useConfigActions> | null = null
  const Comp = defineComponent({
    setup() {
      api = useConfigActions()
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  mounted.push(wrapper)
  return { wrapper, get api() { return api! } }
}

describe('useConfigActions 保存在飞保护（审查#B7）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    m.postConfig.mockImplementation(async () => ({ status: 'ok' }))
    m.confirmAction.mockImplementation(async () => true)
  })

  afterEach(() => {
    mounted.forEach((w) => w.unmount())
    mounted.length = 0
  })

  it('双击保存只触发一次 postConfig，流程结束后 saving 复位可再次保存', async () => {
    const { api } = harness()
    await flushPromises() // 挂载期的 loadConfig

    const first = api.saveConfig()
    // 在飞标志必须同步置位（首次 await 之前），否则双击无法被拦截
    expect(api.saving.value).toBe(true)

    const second = api.saveConfig() // 在飞期间应被直接丢弃
    await Promise.all([first, second])

    expect(m.postConfig).toHaveBeenCalledTimes(1)
    expect(m.confirmAction).toHaveBeenCalledTimes(1)
    expect(api.saving.value).toBe(false)

    // 复位后再次保存应正常工作（保护不是「一次性锁死」）
    await api.saveConfig()
    expect(m.postConfig).toHaveBeenCalledTimes(2)
    expect(api.saving.value).toBe(false)
  })
})

/**
 * E 批：后端保存顺序改为「先落盘、后热重载」后的提示口径。
 *
 * 两种失败必须区分开，否则现场会做出相反的动作：
 * - 落盘失败 → 什么都没改（提示「配置未被修改」，可以修复磁盘后重试）；
 * - 落盘成功但热重载被拒且回滚也失败 → 磁盘上已是新配置（必须告知重启后生效，
 *   否则用户会反复重试并以为保存没成功）。
 */
describe('useConfigActions 保存失败提示（E 批：落盘前置）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    m.postConfig.mockImplementation(async () => ({ status: 'ok' }))
    m.confirmAction.mockImplementation(async () => true)
  })

  afterEach(() => {
    mounted.forEach((w) => w.unmount())
    mounted.length = 0
  })

  it('落盘失败：提示「配置未被修改」', async () => {
    m.postConfig.mockRejectedValueOnce(new Error('save config failed: Read-only file system'))
    const { api } = harness()
    await flushPromises() // 挂载期的 loadConfig

    await api.saveConfig()

    expect(ElMessage.error).toHaveBeenLastCalledWith(expect.stringContaining('配置未被修改'))
    // 不得出现「磁盘已写入」这类反向结论
    expect(ElMessage.error).not.toHaveBeenLastCalledWith(expect.stringContaining('磁盘已写入'))
  })

  it('落盘成功但回滚失败：必须说明磁盘已是新配置、重启后生效', async () => {
    m.postConfig.mockRejectedValueOnce(
      new Error(
        'bus busy (disk holds the new config; rollback of the previous config failed: permission denied)',
      ),
    )
    const { api } = harness()
    await flushPromises()

    await api.saveConfig()

    expect(ElMessage.error).toHaveBeenLastCalledWith(
      expect.stringContaining('重启网关后将按新配置运行'),
    )
    expect(ElMessage.error).not.toHaveBeenLastCalledWith(expect.stringContaining('配置未被修改'))
  })
})
