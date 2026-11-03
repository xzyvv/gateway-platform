import { describe, it, expect } from 'vitest'
import { defineComponent, h, type Ref } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { createRouter, createMemoryHistory, type Router } from 'vue-router'
import { useQueryState } from './useQueryState'

/** 用真实 router（memory history）挂载，才能覆盖 route.query ↔ state 的真实交互 */
async function harness(keys: string[]) {
  const router: Router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/', component: { render: () => null } }],
  })
  await router.push('/')
  await router.isReady()

  const states: Record<string, Ref<string>> = {}
  const Comp = defineComponent({
    setup() {
      for (const k of keys) states[k] = useQueryState(k)
      return () => h('div')
    },
  })
  const wrapper = mount(Comp, { global: { plugins: [router] } })
  return { wrapper, router, states }
}

describe('useQueryState', () => {
  it('单个 key：写入后进入 URL，回到默认值后从 URL 移除', async () => {
    const { router, states } = await harness(['filter'])
    states.filter!.value = 'abc'
    await flushPromises()
    expect(router.currentRoute.value.query.filter).toBe('abc')

    states.filter!.value = ''
    await flushPromises()
    expect(router.currentRoute.value.query.filter).toBeUndefined()
  })

  /**
   * 回归：原实现每个 key 一个 watch 且各自用「当次读到的 route.query」重建整个 query，
   * 同一批写入时后写覆盖先写 —— 只有最后一个 key 能进 URL。
   * History.vue 还原查询条件时正是一次写 5 个 key，故本条即其真实场景。
   */
  it('同一批写入多个 key 时不得丢参', async () => {
    const keys = ['point_id', 'start', 'end', 'downsample', 'granularity']
    const { router, states } = await harness(keys)

    states.point_id!.value = 'p1'
    states.start!.value = '100'
    states.end!.value = '200'
    states.downsample!.value = '5'
    states.granularity!.value = 'hour'
    await flushPromises()

    expect(router.currentRoute.value.query).toMatchObject({
      point_id: 'p1',
      start: '100',
      end: '200',
      downsample: '5',
      granularity: 'hour',
    })
  })

  it('同批写入不破坏 URL 上已存在的其它参数（如 HubShell 的 tab）', async () => {
    const { router, states } = await harness(['level'])
    await router.replace({ query: { tab: 'logs' } })
    await flushPromises()

    states.level!.value = 'error'
    await flushPromises()
    expect(router.currentRoute.value.query).toMatchObject({ tab: 'logs', level: 'error' })
  })

  /** 回归：原实现只写不读（单向），URL 外部变化不会回灌到 state */
  it('URL 外部变化时回灌到 state', async () => {
    const { router, states } = await harness(['level'])
    await router.replace({ query: { level: 'error' } })
    await flushPromises()
    expect(states.level!.value).toBe('error')

    // 前进/后退等场景：URL 回到无该参数，state 也应回到默认值
    await router.replace({ query: {} })
    await flushPromises()
    expect(states.level!.value).toBe('')
  })

  it('默认值不写入 URL，且非空默认值可被 URL 缺省还原', async () => {
    const router = createRouter({
      history: createMemoryHistory(),
      routes: [{ path: '/', component: { render: () => null } }],
    })
    await router.push('/')
    await router.isReady()
    let state!: Ref<string>
    const Comp = defineComponent({
      setup() {
        state = useQueryState('granularity', 'raw')
        return () => h('div')
      },
    })
    const wrapper = mount(Comp, { global: { plugins: [router] } })
    expect(state.value).toBe('raw')
    expect(router.currentRoute.value.query.granularity).toBeUndefined()

    state.value = 'hour'
    await flushPromises()
    expect(router.currentRoute.value.query.granularity).toBe('hour')
    wrapper.unmount()
  })
})
