import { describe, it, expect, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { createPinia, setActivePinia } from 'pinia'
import WsStatusBadge from './WsStatusBadge.vue'
import { useWebsocketStore } from '@/stores/websocket'

/**
 * 回归测试：WS 断开/恢复必须能被读屏器播报（无障碍 #4b）。
 *
 * 历史问题：徽章是 v-if 条件渲染（只在断开/重连时出现），且没有任何
 * aria-live —— 读屏器完全感知不到连接状态变化；连接恢复时徽章卸载，
 * 恢复事件更是无处播报。
 */

function mountBadge() {
  // 不 stub el-dropdown：stub 后默认插槽不渲染，徽章本体（含状态图标）整个消失，
  // 而徽章本体的 aria 语义正是本 spec 要验的
  return mount(WsStatusBadge, {
    global: {
      stubs: { ElDropdownMenu: true, ElDropdownItem: true },
    },
  })
}

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('WsStatusBadge 无障碍播报', () => {
  let store: ReturnType<typeof useWebsocketStore>

  beforeEach(() => {
    setActivePinia(createPinia())
    store = useWebsocketStore()
  })

  it('常驻 live region 始终在 DOM 中（供状态变化播报）', async () => {
    const wrapper = mountBadge()
    await settle()

    const live = wrapper.find('[aria-live]')
    expect(live.exists()).toBe(true)
    expect(live.attributes('role')).toBe('status')
    expect(live.attributes('aria-atomic')).toBe('true')
  })

  it('断连时用 assertive 立即播报，并带上错误原因', async () => {
    store.addActiveStream('/api/realtime')
    store.setLastError('连接被拒绝')
    store.setStatus('disconnected')
    const wrapper = mountBadge()
    await settle()

    const live = wrapper.find('[aria-live]')
    expect(live.attributes('aria-live')).toBe('assertive')
    expect(live.text()).toContain('WebSocket 连接已断开')
    expect(live.text()).toContain('连接被拒绝')
  })

  it('重连进度用 polite 排队播报，不打断当前朗读', async () => {
    store.addActiveStream('/api/realtime')
    store.setStatus('disconnected')
    const wrapper = mountBadge()
    await settle()
    expect(wrapper.find('[aria-live]').attributes('aria-live')).toBe('assertive')

    store.incrementReconnect()
    store.setStatus('connecting')
    await settle()

    const live = wrapper.find('[aria-live]')
    expect(live.attributes('aria-live')).toBe('polite')
    expect(live.text()).toContain('正在重连')
    expect(live.text()).toContain('第 1 次')
  })

  it('连接恢复后播报恢复事件（徽章已卸载，恢复消息仍要送到）', async () => {
    store.addActiveStream('/api/realtime')
    store.setStatus('disconnected')
    const wrapper = mountBadge()
    await settle()
    expect(wrapper.text()).toContain('连接已断开')

    store.setStatus('connected')
    await settle()

    const live = wrapper.find('[aria-live]')
    expect(live.exists()).toBe(true)
    expect(live.text()).toContain('连接已恢复')
    expect(live.attributes('aria-live')).toBe('polite')
  })

  it('无活跃实时流时不播报断连（页面本就没实时功能，提示是噪音）', async () => {
    store.setStatus('disconnected')
    const wrapper = mountBadge()
    await settle()

    const live = wrapper.find('[aria-live]')
    expect(live.text()).not.toContain('连接已断开')
  })

  it('重复断连时文案带自增序号，保证 live region 内容变化触发播报', async () => {
    store.addActiveStream('/api/realtime')
    const wrapper = mountBadge()
    await settle()

    store.setStatus('disconnected')
    await settle()
    const first = wrapper.find('[aria-live]').text()

    store.setStatus('connected')
    await settle()
    store.setStatus('disconnected')
    await settle()
    const second = wrapper.find('[aria-live]').text()

    // 文案必须不同，否则读屏器因内容未变而不播报第二次
    expect(second).not.toBe(first)
    expect(second).toContain('连接已断开')
  })

  it('状态图标标记 aria-hidden（避免读屏器念出无意义的图标名）', async () => {
    store.addActiveStream('/api/realtime')
    store.setStatus('disconnected')
    const wrapper = mountBadge()
    await settle()

    const ico = wrapper.find('.ws-state-ico')
    expect(ico.exists()).toBe(true)
    expect(ico.attributes('aria-hidden')).toBe('true')
  })
})
