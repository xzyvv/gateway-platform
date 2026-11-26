import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import {
  usePolledTask,
  toTaskTerminal,
  isTaskRunning,
  type PolledTaskOptions,
  type PolledTaskReturn,
} from './useTaskProgress'

type Row = { status: string }

function mountTask(opts: PolledTaskOptions<Row>) {
  let api: PolledTaskReturn | undefined
  const Comp = defineComponent({
    setup() {
      api = usePolledTask(opts)
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  return { wrapper, api: api! }
}

describe('useTaskProgress 终态口径', () => {
  it('done/completed 都归为 completed，aborted/stopped/failed 各归其位', () => {
    expect(toTaskTerminal('done')).toBe('completed')
    expect(toTaskTerminal('COMPLETED')).toBe('completed')
    expect(toTaskTerminal('aborted')).toBe('aborted')
    expect(toTaskTerminal('stopped')).toBe('stopped')
    expect(toTaskTerminal('failed')).toBe('failed')
  })

  it('未识别的状态按 failed 收尾（失败不得报成完成）', () => {
    expect(toTaskTerminal('weird')).toBe('failed')
    expect(toTaskTerminal('')).toBe('failed')
    expect(toTaskTerminal(undefined)).toBe('failed')
  })

  it('只有显式 running 算进行中', () => {
    expect(isTaskRunning('running')).toBe(true)
    expect(isTaskRunning(' RUNNING ')).toBe(true)
    expect(isTaskRunning('done')).toBe(false)
    expect(isTaskRunning(undefined)).toBe(false)
  })
})

describe('usePolledTask 轮询收尾', () => {
  beforeEach(() => {
    // 与 usePolling 测试同一策略：只伪造定时器，保留真实 setTimeout
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('running 继续轮询，进入终态后只收尾一次并停表', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce({ status: 'running' })
      .mockResolvedValueOnce({ status: 'done' })
    const onUpdate = vi.fn()
    const onSettled = vi.fn()
    const onFetchFailures = vi.fn()
    const { wrapper, api } = mountTask({
      interval: 1000,
      fetch,
      statusOf: (r) => r.status,
      onUpdate,
      onSettled,
      onFetchFailures,
    })

    api.begin(true)
    await flushPromises()
    expect(onSettled).not.toHaveBeenCalled()
    expect(api.polling.value).toBe(true)

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(onUpdate).toHaveBeenCalledTimes(2)
    expect(onSettled).toHaveBeenCalledTimes(1)
    expect(onSettled.mock.calls[0]?.[1]).toBe('completed')
    expect(api.polling.value).toBe(false)

    // 收尾后不再发请求
    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(fetch).toHaveBeenCalledTimes(2)
    expect(onSettled).toHaveBeenCalledTimes(1)
    wrapper.unmount()
  })

  it('状态为 failed/aborted 时按终态收尾，不当作进行中', async () => {
    for (const status of ['failed', 'aborted']) {
      const fetch = vi.fn().mockResolvedValue({ status })
      const onSettled = vi.fn()
      const { wrapper, api } = mountTask({
        interval: 1000,
        fetch,
        statusOf: (r) => r.status,
        onSettled,
        onFetchFailures: vi.fn(),
      })
      api.begin(true)
      await flushPromises()
      expect(onSettled).toHaveBeenCalledTimes(1)
      expect(onSettled.mock.calls[0]?.[1]).toBe(status)
      wrapper.unmount()
    }
  })

  it('连续 3 次拉取失败后收尾，此后不再轮询', async () => {
    const fetch = vi.fn().mockRejectedValue(new Error('网关掉线'))
    const onFetchFailures = vi.fn()
    const onSettled = vi.fn()
    const { wrapper, api } = mountTask({
      interval: 1000,
      fetch,
      statusOf: (r) => r.status,
      onSettled,
      onFetchFailures,
    })

    api.begin(true)
    await flushPromises()
    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(onFetchFailures).not.toHaveBeenCalled()

    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(onFetchFailures).toHaveBeenCalledTimes(1)
    expect(onSettled).not.toHaveBeenCalled()
    expect(api.polling.value).toBe(false)

    vi.advanceTimersByTime(5000)
    await flushPromises()
    expect(fetch).toHaveBeenCalledTimes(3)
    wrapper.unmount()
  })

  it('requireResult=false 时空结果继续轮询；true 时计入失败', async () => {
    const fetch = vi.fn().mockResolvedValue(null)
    const onFetchFailures = vi.fn()
    const { wrapper, api } = mountTask({
      interval: 1000,
      fetch,
      statusOf: (r) => r.status,
      onSettled: vi.fn(),
      onFetchFailures,
    })
    api.begin(true)
    await flushPromises()
    vi.advanceTimersByTime(3000)
    await flushPromises()
    expect(onFetchFailures).not.toHaveBeenCalled()
    expect(api.polling.value).toBe(true)
    wrapper.unmount()

    const fetch2 = vi.fn().mockResolvedValue(null)
    const fail2 = vi.fn()
    const second = mountTask({
      interval: 1000,
      fetch: fetch2,
      statusOf: (r) => r.status,
      onSettled: vi.fn(),
      onFetchFailures: fail2,
      requireResult: true,
    })
    second.api.begin(true)
    await flushPromises()
    vi.advanceTimersByTime(1000)
    await flushPromises()
    vi.advanceTimersByTime(1000)
    await flushPromises()
    expect(fail2).toHaveBeenCalledTimes(1)
    second.wrapper.unmount()
  })

  it('end() 后迟到响应不得再收尾', async () => {
    let resolveFetch: ((v: Row) => void) | undefined
    const fetch = vi.fn(
      () => new Promise<Row>((resolve) => {
        resolveFetch = resolve
      }),
    )
    const onSettled = vi.fn()
    const { wrapper, api } = mountTask({
      interval: 1000,
      fetch,
      statusOf: (r) => r.status,
      onSettled,
      onFetchFailures: vi.fn(),
    })

    api.begin(true)
    await flushPromises()
    api.end()
    expect(api.polling.value).toBe(false)
    resolveFetch?.({ status: 'done' })
    await flushPromises()
    expect(onSettled).not.toHaveBeenCalled()
    wrapper.unmount()
  })
})
