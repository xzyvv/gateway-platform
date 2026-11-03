import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { createPinia, setActivePinia } from 'pinia'
import { nextTick } from 'vue'
import ElementPlus from 'element-plus'

import RequestMonitor from './RequestMonitor.vue'
import { useRequestLogStore } from '../stores/requestLog'

/** 捕获「复制为 cURL」写入剪贴板的内容 */
const copiedText = vi.hoisted(() => ({ value: '' }))
vi.mock('../utils/clipboard', () => ({
  copyText: async (text: string) => {
    copiedText.value = text
  },
}))

/**
 * 请求监视器面板的 selected 悬空问题。
 *
 * selected 持有日志条目引用，而 store 上限 200 条、超出即 FIFO 淘汰。原实现
 * 从不校验引用是否还在列表里，于是会出现三种"悬空选中"：
 *   1. 关闭面板后再打开，详情还挂着上次的记录
 *   2. 选中后条目被新记录挤出（超过 200 条），详情仍在但列表里找不到
 *   3. 点「清空」后详情仍指向已删除的条目
 * 这些都会让用户以为监视器坏了。
 */
function makeEntry(id: number, url = `/api/x/${id}`) {
  return {
    id,
    method: 'GET',
    url,
    status: 200,
    duration: 12,
    timestamp: 1_700_000_000_000 + id,
    startTime: 1_700_000_000_000 + id,
    endTime: 1_700_000_000_012 + id,
  }
}

/**
 * 本文件挂载过的组件统一登记：用例结束必须卸载。
 *
 * 面板可见时组件会起 rAF + 1s interval 采样，卸载才走 onUnmounted(stopPerfSampling)。
 * 早期用例不卸载，组件会带着 `_samplingActive=true` 活到下一个用例——切换
 * fake/real timers 后残留回调会继续跑，导致「已停止采样」的断言偶发多出几条记录。
 */
const mountedWrappers: Array<{ unmount: () => void }> = []
function track<T extends { unmount: () => void }>(wrapper: T): T {
  mountedWrappers.push(wrapper)
  return wrapper
}
function unmountAll() {
  for (const w of mountedWrappers.splice(0)) w.unmount()
}

function setup() {
  const pinia = createPinia()
  setActivePinia(pinia)
  const store = useRequestLogStore()
  const wrapper = track(mount(RequestMonitor, {
    global: { plugins: [pinia, ElementPlus] },
  }))
  return { wrapper, store }
}

describe('RequestMonitor 详情选中态', () => {
  afterEach(() => {
    unmountAll()
    vi.useRealTimers()
  })

  it('未选中时不显示详情；点击行后显示', async () => {
    const { wrapper, store } = setup()
    store.pushLog(makeEntry(1))
    await nextTick()

    expect(wrapper.find('.rm-detail').exists()).toBe(false)

    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    await wrapper.find('.rm-row').trigger('click')
    await nextTick()

    expect(wrapper.find('.rm-detail').exists()).toBe(true)
    expect(wrapper.find('.rm-detail').text()).toContain('/api/x/1')
  })

  it('关闭面板后重新打开，详情已收起（不残留上次选中）', async () => {
    const { wrapper, store } = setup()
    store.pushLog(makeEntry(1))
    await nextTick()

    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    await wrapper.find('.rm-row').trigger('click')
    await nextTick()
    expect(wrapper.find('.rm-detail').exists()).toBe(true)

    // 关闭面板（最后一个按钮，无文字）
    const closeBtn = wrapper.findAll('.rm-actions button').at(-1)!
    await closeBtn.trigger('click')
    await nextTick()
    expect(wrapper.find('.rm-panel').exists()).toBe(false)

    // 再次打开：详情不应残留
    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    expect(wrapper.find('.rm-detail').exists()).toBe(false)
  })

  it('选中后条目被 FIFO 淘汰：详情自动收起', async () => {
    const { wrapper, store } = setup()
    store.pushLog(makeEntry(1))
    await nextTick()

    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    await wrapper.find('.rm-row').trigger('click')
    await nextTick()
    expect(wrapper.find('.rm-detail').exists()).toBe(true)

    // 灌满 200 条把 id=1 挤出去
    for (let i = 2; i <= 205; i++) store.pushLog(makeEntry(i))
    await nextTick()

    expect(wrapper.find('.rm-detail').exists()).toBe(false)
  })

  it('点击「清空」后详情收起', async () => {
    const { wrapper, store } = setup()
    store.pushLog(makeEntry(1))
    await nextTick()

    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    await wrapper.find('.rm-row').trigger('click')
    await nextTick()
    expect(wrapper.find('.rm-detail').exists()).toBe(true)

    const clearBtn = wrapper.findAll('.rm-actions button').find((b) => b.text().includes('清空'))
    await clearBtn!.trigger('click')
    await nextTick()

    expect(wrapper.find('.rm-detail').exists()).toBe(false)
  })
})

/**
 * 性能采样（FPS + JS Heap）测试：
 *
 * 浏览器环境差异处理：
 *   - jsdom 没有 performance.memory → 用 Object.defineProperty 注入 mock
 *   - vi.useFakeTimers 会劫持 requestAnimationFrame / performance.now / setInterval / clearInterval
 *   - 面板 open/close 用 setProps({ open: true/false }) 而非模板点击触发，
 *     这样 watch(open) 在 fake timers 下也能可靠 flush（不依赖 Vue event + microtask 时机）
 *   - 不直接 spy rAF/cancelAnimationFrame（fake timers 接管后 spy 不准），
 *     通过观测 memHistory / fpsHistory 长度变化间接验证启停
 */
describe('RequestMonitor 性能采样', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    Object.defineProperty(performance, 'memory', {
      value: { usedJSHeapSize: 50_000_000, totalJSHeapSize: 100_000_000 },
      configurable: true,
    })
  })

  afterEach(() => {
    // 先卸载（此时仍是 fake timers，clearInterval 才能命中采样定时器），再重置时钟
    unmountAll()
    vi.clearAllTimers()
    vi.useRealTimers()
    delete (performance as { memory?: unknown }).memory
  })

  /** 统一用 props 驱动 open/close：fake timers 下 Vue watch flush 才可靠 */
  function setupWithOpen(initialOpen = false) {
    const pinia = createPinia()
    setActivePinia(pinia)
    const store = useRequestLogStore()
    const wrapper = track(mount(RequestMonitor, {
      props: { open: initialOpen },
      global: { plugins: [pinia, ElementPlus] },
    }))
    return { wrapper, store }
  }

  it('面板 open 时启动采样：内存立即可用，FPS/内存历史队列开始累积', async () => {
    const { wrapper } = setupWithOpen(false)

    // 初始状态：采样未启动
    expect(wrapper.vm.currentFps).toBe(0)
    expect(wrapper.vm.currentMemPct).toBe(null)
    expect(wrapper.vm.fpsHistory.length).toBe(0)
    expect(wrapper.vm.memHistory.length).toBe(0)

    // 用 props 打开面板 → watch(open) → startPerfSampling
    await wrapper.setProps({ open: true })
    await nextTick()

    // startPerfSampling 内先 sampleMem() 一次 → 立即有值
    expect(wrapper.vm.currentMemPct).toBe(50)
    expect(wrapper.vm.memHistory).toEqual([50])

    // 推进 3 秒 → interval 触发至少 2 次
    vi.advanceTimersByTime(3000)
    await nextTick()

    expect(wrapper.vm.memHistory.length).toBeGreaterThanOrEqual(3)
    expect(wrapper.vm.fpsHistory.length).toBeGreaterThan(0)
  })

  it('面板 close 时停止采样：memHistory 不再增长（stopPerfSampling 内 clearInterval 生效）', async () => {
    const { wrapper } = setupWithOpen(false)

    // 手动启动采样（绕过 Vue watch 的 flush 时机，fake timers 下更可靠）
    wrapper.vm.startPerfSampling()
    await nextTick()
    vi.advanceTimersByTime(2000)
    await nextTick()

    const memLenBefore = wrapper.vm.memHistory.length
    expect(memLenBefore).toBeGreaterThan(0)

    // 手动停止采样（等价于 close 面板触发 stopPerfSampling）
    wrapper.vm.stopPerfSampling()
    // 机制层证据：rAF 与 1s interval 都已注销（只剩长度断言时，若定时器泄漏
    // 会表现为「多出几条记录」，排查成本高）
    expect(vi.getTimerCount()).toBe(0)

    // 再推进 5 秒 → interval 已被 clear，memHistory 应不再增长
    vi.advanceTimersByTime(5000)
    await nextTick()

    expect(wrapper.vm.memHistory.length).toBe(memLenBefore)

    // FPS 停止累积：runAllTimers 刷完残留 rAF tick（被 _samplingActive=false 守卫拦截）
    vi.runAllTimers()
    await nextTick()
    const fpsLenAfter = wrapper.vm.fpsHistory.length
    vi.advanceTimersByTime(3000)
    vi.runAllTimers()
    await nextTick()
    expect(wrapper.vm.fpsHistory.length).toBe(fpsLenAfter)
  })

  it('FPS/内存历史队列 cap 30，超出后 FIFO 淘汰最早一条', async () => {
    const { wrapper } = setupWithOpen(false)

    wrapper.vm.startPerfSampling()
    await nextTick()

    // 推进 60 秒 → interval 触发 ~60 次，但队列 cap 30
    for (let i = 0; i < 60; i++) {
      vi.advanceTimersByTime(1000)
      await nextTick()
    }

    expect(wrapper.vm.fpsHistory.length).toBeLessThanOrEqual(30)
    expect(wrapper.vm.memHistory.length).toBe(30)
  })
})

/**
 * 「复制为 cURL」的回归护栏（P1-12）。
 *
 * 背景：原实现只输出「方法 + URL + Body」并注明「请求头未记录」，
 * 于是带鉴权的接口在终端必然 401——「复制出来的东西用不了」。
 * 现在必须带上实际请求头，且 URL 是绝对地址（否则粘到终端是相对路径，直接失败）。
 */
describe('RequestMonitor 复制为 cURL', () => {
  beforeEach(() => {
    copiedText.value = ''
  })

  afterEach(() => {
    unmountAll()
    vi.useRealTimers()
  })

  it('带上实际请求头：Content-Type / Accept 出现在命令里', async () => {
    const { wrapper, store } = setup()
    store.pushLog({
      ...makeEntry(1),
      method: 'POST',
      url: '/api/config',
      requestData: { mqtt: { host: '10.0.0.1' } },
      requestHeaders: [
        { name: 'Accept', value: 'application/json' },
        { name: 'Content-Type', value: 'application/json' },
      ],
    })
    await nextTick()
    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    await wrapper.find('.rm-row').trigger('click')
    await nextTick()
    const btn = wrapper.findAll('.rm-detail-actions button').find((b) => b.text().includes('cURL'))
    await btn!.trigger('click')
    await nextTick()

    expect(copiedText.value).toContain("-H 'Accept: application/json'")
    expect(copiedText.value).toContain("-H 'Content-Type: application/json'")
    expect(copiedText.value).toContain('-d ')
    expect(copiedText.value).toContain('"host":"10.0.0.1"')
  })

  it('URL 补成绝对地址：终端里相对路径必然失败', async () => {
    const { wrapper, store } = setup()
    store.pushLog(makeEntry(1))
    await nextTick()
    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    await wrapper.find('.rm-row').trigger('click')
    await nextTick()
    const btn = wrapper.findAll('.rm-detail-actions button').find((b) => b.text().includes('cURL'))
    await btn!.trigger('click')
    await nextTick()

    // 日志里是相对路径 /api/x/1，命令里必须是绝对地址
    expect(copiedText.value).toContain("curl -X GET '")
    expect(copiedText.value).toContain('/api/x/1')
    expect(copiedText.value).toMatch(/curl -X GET 'https?:\/\/[^']+\/api\/x\/1'/)
  })

  it('Authorization 已脱敏：给出占位符与提示，不落明文 token', async () => {
    const { wrapper, store } = setup()
    store.pushLog({
      ...makeEntry(1),
      requestHeaders: [{ name: 'Authorization', value: '***' }],
    })
    await nextTick()
    await wrapper.find('.rm-fab').trigger('click')
    await nextTick()
    await wrapper.find('.rm-row').trigger('click')
    await nextTick()
    const btn = wrapper.findAll('.rm-detail-actions button').find((b) => b.text().includes('cURL'))
    await btn!.trigger('click')
    await nextTick()

    expect(copiedText.value).toContain("-H 'Authorization: Bearer <你的 token>'")
    expect(copiedText.value).toContain('请把 <你的 token> 换成实际 token')
    // 绝不能出现明文 token
    expect(copiedText.value).not.toMatch(/Bearer [A-Za-z0-9_-]{16,}/)
  })
})