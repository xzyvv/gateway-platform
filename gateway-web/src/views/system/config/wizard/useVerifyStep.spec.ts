import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount } from '@vue/test-utils'

/**
 * 部署向导第 5 步（验证采集）的契约。
 *
 * 这一步是现场「装完能不能用」的唯一判据，历史上最容易掺「假成功」兜底：
 * 读不到就显示成功、或把失败项从列表里滤掉，用户以为通了，上线后才发现没数据。
 * 因此这里锁住三件事：
 * 1. 每个点位的成败**逐条**进列表，不合并、不丢弃；
 * 2. 结论文案必须同时给出成功数与失败数，并提示往哪查（从站地址/寄存器/类型）；
 * 3. 一个都没成功时 `verified` 必须为 false——否则「下一步」按钮会放行。
 */
const readSequential = vi.hoisted(() => vi.fn())

vi.mock('@/composables/data/useReadPointNow', () => ({
  useReadPointNow: () => ({ readSequential }),
}))

const messageMock = vi.hoisted(() => ({
  warning: vi.fn(),
  success: vi.fn(),
  error: vi.fn(),
}))
vi.mock('element-plus', () => ({ ElMessage: messageMock }))

import { useVerifyStep } from './useVerifyStep'
import type { PointConfig } from '@/api/types'

/** 在真实组件实例里跑 composable，才能验证 onUnmounted 的行为 */
function withSetup() {
  let result!: ReturnType<typeof useVerifyStep>
  const wrapper = mount(
    defineComponent({
      setup() {
        result = useVerifyStep()
        return () => h('div')
      },
    }),
  )
  return { result, wrapper }
}

function point(id: string, name = ''): PointConfig {
  return { id, name } as PointConfig
}

const ok = (pointId: string, value: number) => ({ pointId, ok: true, value, status: 'ok', error: '' })

beforeEach(() => {
  vi.clearAllMocks()
})

describe('useVerifyStep', () => {
  it('没有点位时不发起读取，只提示', async () => {
    const { result } = withSetup()
    await result.verify([])

    expect(readSequential).not.toHaveBeenCalled()
    expect(messageMock.warning).toHaveBeenCalledWith('没有可读的点位')
    expect(result.verifying.value).toBe(false)
  })

  it('全部读取成功：结论为全部成功且放行', async () => {
    readSequential.mockImplementation(async (ids: string[], opts: { onResult: (o: unknown) => void }) => {
      ids.forEach((id, i) => opts.onResult(ok(id, 10 + i)))
    })
    const { result } = withSetup()
    await result.verify([point('p1', '电压'), point('p2', '电流'), point('p3', '功率')])

    expect(result.verified.value).toBe(true)
    expect(result.verifyFailed.value).toBe(false)
    expect(result.verifySummary.value).toContain('3 个点位全部读取成功')
    expect(result.readResults.value.map((r) => r.name)).toEqual(['电压', '电流', '功率'])
    expect(messageMock.success).toHaveBeenCalledWith('已读取全部点位')
  })

  it('部分失败：逐条列出、结论带成功与失败数，且提示核查方向', async () => {
    readSequential.mockImplementation(async (ids: string[], opts: { onResult: (o: unknown) => void }) => {
      opts.onResult(ok(ids[0]!, 1))
      opts.onResult({ pointId: ids[1]!, ok: false, value: null, status: 'error', error: '从站无响应' })
      opts.onResult(ok(ids[2]!, 3))
    })
    const { result } = withSetup()
    await result.verify([point('p1'), point('p2'), point('p3')])

    expect(result.verifyFailed.value).toBe(true)
    // 只要有一个成功，就允许继续走下一步（部分点位可用的现场很常见）
    expect(result.verified.value).toBe(true)
    expect(result.verifySummary.value).toContain('成功 2 个，失败 1 个')
    expect(result.verifySummary.value).toContain('从站地址')
    expect(result.readResults.value[1]!.value).toBe('从站无响应')
    // 名称缺失时回退到点位 id，避免列表出现空行
    expect(result.readResults.value[0]!.name).toBe('p1')
    expect(messageMock.warning).toHaveBeenCalledWith('部分点位读取失败，请按提示核对配置')
  })

  it('全部失败：不给「已验证」结论，失败项用「无响应」兜底', async () => {
    readSequential.mockImplementation(async (ids: string[], opts: { onResult: (o: unknown) => void }) => {
      ids.forEach((id) => opts.onResult({ pointId: id, ok: false, value: null, status: '', error: '' }))
    })
    const { result } = withSetup()
    await result.verify([point('p1')])

    expect(result.verified.value).toBe(false)
    expect(result.verifyFailed.value).toBe(true)
    expect(result.readResults.value[0]!.value).toBe('无响应')
    expect(messageMock.success).not.toHaveBeenCalled()
  })

  it('重复验证会清掉上一轮结果，不把新旧点位混在一起', async () => {
    readSequential.mockImplementation(async (ids: string[], opts: { onResult: (o: unknown) => void }) => {
      opts.onResult(ok(ids[0]!, 1))
    })
    const { result } = withSetup()
    await result.verify([point('p1')])
    await result.verify([point('p2')])

    expect(result.readResults.value).toHaveLength(1)
    expect(result.readResults.value[0]!.id).toBe('p2')
  })

  it('页面卸载后不再回写结论（避免对着已销毁的页面弹提示）', async () => {
    let release!: () => void
    const gate = new Promise<void>((r) => { release = r })
    readSequential.mockImplementation(async (ids: string[], opts: { onResult: (o: unknown) => void }) => {
      opts.onResult(ok(ids[0]!, 1))
      await gate
    })

    const { result, wrapper } = withSetup()
    const running = result.verify([point('p1')])
    wrapper.unmount()
    release()
    await running

    expect(result.verifySummary.value).toBe('')
    expect(result.verified.value).toBe(false)
    expect(messageMock.success).not.toHaveBeenCalled()
  })
})
