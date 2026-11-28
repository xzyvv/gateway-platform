import { describe, it, expect, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import NotificationCenter from './NotificationCenter.vue'
import { useNotificationsStore } from '@/stores/notifications'

/**
 * 通知中心的行为契约：
 *   写操作失败全局静默后，失败信息只能在这里查到 —— 所以「有未读角标」「打开
 *   能看到条目」「打开即已读」这三件事必须成立，否则静默就变成了信息丢失。
 *
 * 注意：抽屉开了 append-to-body（顶栏 backdrop-filter 会让 fixed 抽屉被
 * 困在 60px 顶栏里），所以抽屉内容 teleport 到 document.body —— 断言要走
 * body 查询，不能再用 wrapper.find。
 */
function mountCenter() {
  return mount(NotificationCenter, { attachTo: document.body })
}

describe('NotificationCenter', () => {
  beforeEach(() => {
    document.body.innerHTML = ''
  })

  it('静默错误只进队列时，铃铛出现未读角标', () => {
    const store = useNotificationsStore()
    store.error('保存自愈配置失败(500)', { silent: true })

    const wrapper = mountCenter()
    expect(store.items).toHaveLength(1)
    expect(wrapper.find('.el-badge__content').text()).toBe('1')
    expect(wrapper.get('.icon-btn').attributes('aria-label')).toContain('1 条未读')
    wrapper.unmount()
  })

  it('打开抽屉能看到消息全文，并把未读清零', async () => {
    const store = useNotificationsStore()
    store.error('保存自愈配置失败(500)', { silent: true })

    const wrapper = mountCenter()
    await wrapper.get('.icon-btn').trigger('click')
    await nextTick()
    await flushPromises()

    const msg = document.body.querySelector('.notice-msg')
    expect(msg).toBeTruthy()
    expect(msg!.textContent).toContain('保存自愈配置失败')
    expect(store.unreadCount).toBe(0)
    wrapper.unmount()
  })

  it('清空后回到空状态', async () => {
    const store = useNotificationsStore()
    store.error('请求失败(500): boom', { silent: true })

    const wrapper = mountCenter()
    await wrapper.get('.icon-btn').trigger('click')
    await nextTick()
    await flushPromises()

    const clearBtn = Array.from(document.body.querySelectorAll('button')).find(
      (b) => b.textContent?.trim() === '清空',
    ) as HTMLButtonElement | undefined
    expect(clearBtn).toBeTruthy()
    clearBtn!.click()
    await nextTick()
    await flushPromises()

    expect(store.items).toHaveLength(0)
    expect(document.body.querySelector('.notice-item')).toBeNull()
    wrapper.unmount()
  })
})
