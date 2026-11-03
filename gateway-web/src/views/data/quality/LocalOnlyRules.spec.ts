/**
 * 数据质量规则来源标注防回归
 *
 * 背景（B4 规则落库后）：
 *   「数据质量 › 质量规则配置」的规则正常情况下落库到网关（/api/quality/rules），
 *   写成功后网关把启用规则安装到处理管道，range/rate/nan 真实执行。
 *   仅当后端不可用（离线 / 老版本网关）时回退 localStorage。
 *
 * 本 spec 锁定两条不变量：
 *   1. local（本机回退）→ 顶部挂 LocalOnlyBadge（「规则仅本机」），说明网关不执行；
 *   2. remote（远端已落库）→ 不出现 LocalOnlyBadge，文案改为「远端已落库 / 规则已下发网关执行」。
 * 任何人删除来源区分或误把本地回退当成远端落库，测试即红。
 */
import { describe, it, expect, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: vi.fn(), replace: vi.fn() }),
  useRoute: () => ({ query: {}, path: '/data' }),
}))

// 规则来源可在用例间切换（hoisted 保证 mock 工厂能引用）
const { sourceState } = vi.hoisted(() => ({
  sourceState: { value: 'local' as 'remote' | 'local' },
}))

vi.mock('@/api/dataQuality', () => ({
  listQualityRules: async () => [],
  createQualityRule: async () => ({}),
  updateQualityRule: async () => undefined,
  deleteQualityRule: async () => undefined,
  qualityRulesSource: () => sourceState.value,
  listPointQuality: async () => [],
  getQualityStats: async () => ({ good: 0, bad: 0, stale: 0, unknown: 0, total: 0 }),
  QUALITY_LABEL: { good: 'good', bad: 'bad', uncertain: 'uncertain', old: 'old' },
  QUALITY_COLOR: { good: 'success', bad: 'danger', uncertain: 'warning', old: 'info' },
}))

vi.mock('@/api/devices', () => ({ listDevices: async () => [] }))
vi.mock('@/api/points', () => ({ listPoints: async () => [] }))

import DataQuality from './DataQuality.vue'

describe('数据质量规则来源标注', () => {
  it('本地回退时挂 LocalOnlyBadge 并说明规则仅存本机、网关不执行', async () => {
    sourceState.value = 'local'
    const wrapper = mount(DataQuality, { global: { stubs: { DataTable: true, 'el-dialog': true } } })
    await flushPromises()
    expect(wrapper.text()).toContain('规则仅本机')
    const badge = wrapper.findComponent({ name: 'LocalOnlyBadge' })
    expect(badge.exists()).toBe(true)
    // 说明文案在 tooltip 里（不渲染为文本），断言 prop
    expect(String(badge.props('tip'))).toContain('网关不加载、不执行')
    wrapper.unmount()
  })

  it('远端落库时不显示 LocalOnlyBadge，并标注规则已下发网关执行', async () => {
    sourceState.value = 'remote'
    const wrapper = mount(DataQuality, { global: { stubs: { DataTable: true, 'el-dialog': true } } })
    await flushPromises()
    expect(wrapper.text()).toContain('远端已落库')
    expect(wrapper.text()).toContain('规则已下发网关执行')
    expect(wrapper.findComponent({ name: 'LocalOnlyBadge' }).exists()).toBe(false)
    wrapper.unmount()
  })
})
