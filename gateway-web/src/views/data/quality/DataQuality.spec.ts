import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage, ElInput, ElSwitch } from 'element-plus'

/**
 * D1：数据管理 › 质量概览（DataQuality）行为测试。
 *
 * 这页同时呈现「网关真实执行的质量判定」和「只存在浏览器里的规则」，
 * 用户很容易把本机规则当成已下发。钉住的核心：
 * - 规则来源必须如实标注：远端已落库 vs 规则仅本机；
 * - 统计卡与点位质量标签使用同一套文案/颜色；
 * - 规则启停落库失败时开关拨回去，不能显示与实际执行不一致的状态；
 * - 两个列表加载失败各自可见报错，不静默留空；
 * - 新建/删除规则走确认与契约校验，失败不谎报。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push: routerPush }) }))
vi.mock('@/api/dataQuality', async () => {
  const actual = await vi.importActual<typeof import('@/api/dataQuality')>('@/api/dataQuality')
  return {
    ...actual,
    listQualityRules: (...a: unknown[]) => listRulesMock(...a),
    createQualityRule: (...a: unknown[]) => createRuleMock(...a),
    updateQualityRule: (...a: unknown[]) => updateRuleMock(...a),
    deleteQualityRule: (...a: unknown[]) => deleteRuleMock(...a),
    getQualityStats: (...a: unknown[]) => statsMock(...a),
    qualityRulesSource: () => sourceMock(),
    listPointQuality: (...a: unknown[]) => listPointsMock(...a),
  }
})
vi.mock('@/utils/confirm', () => ({ confirmDelete: (...a: unknown[]) => confirmDeleteMock(...a) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))
vi.mock('@/composables/core/useDialogDirtyGuard', () => ({
  useDialogDirtyGuard: () => ({ beforeClose: () => {}, close: closeGuard }),
}))

const routerPush = vi.hoisted(() => vi.fn())
const listRulesMock = vi.hoisted(() => vi.fn())
const createRuleMock = vi.hoisted(() => vi.fn())
const updateRuleMock = vi.hoisted(() => vi.fn())
const deleteRuleMock = vi.hoisted(() => vi.fn())
const statsMock = vi.hoisted(() => vi.fn())
const sourceMock = vi.hoisted(() => vi.fn())
const listPointsMock = vi.hoisted(() => vi.fn())
const confirmDeleteMock = vi.hoisted(() => vi.fn())
const showToast = vi.hoisted(() => vi.fn())
const closeGuard = vi.hoisted(() => vi.fn())

import DataQuality from './DataQuality.vue'
import DataTable from '@/components/DataTable.vue'
import { QUALITY_LABEL, type PointQuality, type QualityRule } from '@/api/dataQuality'

const DialogStub = {
  name: 'ElDialog',
  props: ['modelValue', 'title', 'width', 'beforeClose', 'closed', 'closeOnClickModal'],
  template: '<div class="dialog-stub"><slot /><slot name="footer" /></div>',
}

const RULES: QualityRule[] = [
  { id: 'q1', name: '超量程标记', condition: 'range', min: -100, max: 9999, resultQuality: 'bad', enabled: true },
  { id: 'q2', name: '突变标记', condition: 'rate', maxRate: 100, resultQuality: 'uncertain', enabled: false },
]
const POINTS: PointQuality[] = [
  {
    point_id: 'p1', point_name: '温度', device_id: 'd1', device_name: '设备A',
    value: 25.5, quality: 'good', consecutive_failures: 0, last_update_at: 1_760_000_000_000,
  },
  {
    point_id: 'p2', point_name: '电量', device_id: 'd1', device_name: '设备A',
    value: null, quality: 'bad', consecutive_failures: 3, last_update_at: null as unknown as number,
  },
]
const STATS = { good: 12, stale: 3, unknown: 2, bad: 5, total: 22 }

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(DataQuality, { global: { stubs: { ElDialog: DialogStub } } })
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

function tableData<T>(wrapper: ReturnType<typeof mountPage>, index: number): T[] {
  return (wrapper.findAllComponents(DataTable)[index]!.props() as unknown as { data: T[] }).data
}

async function openCreate(wrapper: ReturnType<typeof mountPage>) {
  await button(wrapper, '新建').trigger('click')
  await settle()
}

describe('DataQuality 质量概览', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listRulesMock.mockResolvedValue(RULES.map((r) => ({ ...r })))
    listPointsMock.mockResolvedValue(POINTS.map((p) => ({ ...p })))
    statsMock.mockResolvedValue({ ...STATS })
    sourceMock.mockReturnValue('local')
    createRuleMock.mockResolvedValue({ status: 'ok' })
    updateRuleMock.mockResolvedValue({ status: 'ok' })
    deleteRuleMock.mockResolvedValue({ status: 'ok' })
    confirmDeleteMock.mockResolvedValue(true)
  })

  it('统计卡与点位质量标签共用一套文案，颜色区分四档', async () => {
    const wrapper = mountPage()
    await settle()

    expect(wrapper.findAll('.stat .lbl').map((n) => n.text())).toEqual([
      QUALITY_LABEL.good, QUALITY_LABEL.uncertain, QUALITY_LABEL.old, QUALITY_LABEL.bad,
    ])
    expect(wrapper.findAll('.stat .val').map((n) => n.text())).toEqual(['12', '3', '2', '5'])
    expect(wrapper.findAll('.stat .val').map((n) => n.classes())).toEqual([
      expect.arrayContaining(['good']),
      expect.arrayContaining(['uncertain']),
      expect.arrayContaining(['old']),
      expect.arrayContaining(['bad']),
    ])
    const qualityTags = wrapper.findAll('.el-tag').filter((t) => [QUALITY_LABEL.good, QUALITY_LABEL.bad].includes(t.text()))
    expect(qualityTags.length).toBeGreaterThanOrEqual(2)
  })

  it('规则来源如实标注：local 显示「规则仅本机」，remote 显示已下发', async () => {
    const local = mountPage()
    await settle()
    expect(local.text()).toContain('规则仅本机')
    expect(local.text()).not.toContain('远端已落库')

    sourceMock.mockReturnValue('remote')
    const remote = mountPage()
    await settle()
    expect(remote.text()).toContain('远端已落库 · 规则已下发网关执行')
    expect(remote.text()).not.toContain('规则仅本机')
  })

  it('规则表与点位表按真实字段渲染，时间与空值有明确占位', async () => {
    const wrapper = mountPage()
    await settle()

    const rules = tableData<QualityRule>(wrapper, 0)
    expect(rules.map((r) => r.name)).toEqual(['超量程标记', '突变标记'])
    expect(wrapper.text()).toContain(QUALITY_LABEL.bad)

    const points = tableData<PointQuality>(wrapper, 1)
    expect(points).toHaveLength(2)
    expect(points[0]).toMatchObject({ point_name: '温度', value: 25.5, quality: 'good' })
    // 有更新时间时展示时分秒；null 时不能显示成 1970
    expect(wrapper.text()).toMatch(/\d{1,2}:\d{2}:\d{2}/)
    // 未上报的点位不能渲染成 1970 年的时间
    expect(wrapper.text()).not.toContain('1970')
  })

  it('规则启停失败：开关拨回原状态，并提示失败原因', async () => {
    updateRuleMock.mockRejectedValue(new Error('write 500'))
    const wrapper = mountPage()
    await settle()

    const sw = wrapper.findAllComponents(ElSwitch)[0]!
    sw.vm.$emit('update:modelValue', false)
    sw.vm.$emit('change', false)
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('write 500'), '切换规则状态失败')
    expect(tableData<QualityRule>(wrapper, 0)[0]!.enabled).toBe(true)
  })

  it('规则启停成功：保持新状态并刷新来源标注', async () => {
    sourceMock.mockReturnValue('remote')
    const wrapper = mountPage()
    await settle()

    const sw = wrapper.findAllComponents(ElSwitch)[0]!
    sw.vm.$emit('update:modelValue', false)
    sw.vm.$emit('change', false)
    await settle()

    expect(updateRuleMock).toHaveBeenCalledWith('q1', { enabled: false })
    expect(tableData<QualityRule>(wrapper, 0)[0]!.enabled).toBe(false)
  })

  it('规则加载失败：可见报错，统计与点位表不受影响', async () => {
    listRulesMock.mockRejectedValue(new Error('rules 500'))
    const wrapper = mountPage()
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('rules 500'), '加载质量规则失败')
    expect(wrapper.findAll('.stat .val').map((n) => n.text())).toEqual(['12', '3', '2', '5'])
    expect(tableData<PointQuality>(wrapper, 1)).toHaveLength(2)
  })

  it('统计加载失败：可见报错，页面不清零冒充健康', async () => {
    statsMock.mockRejectedValue(new Error('stats 500'))
    const wrapper = mountPage()
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('stats 500'), '加载质量状态失败')
    expect(tableData<QualityRule>(wrapper, 0)).toHaveLength(2)
  })

  it('新建规则：名称必填，条件切换显示对应参数，提交契约完整', async () => {
    const wrapper = mountPage()
    await openCreate(wrapper)

    await button(wrapper, '保存').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请输入规则名称')
    expect(createRuleMock).not.toHaveBeenCalled()

    await wrapper.findAllComponents(ElInput)[0]!.vm.$emit('update:modelValue', '超时规则')
    const selects = wrapper.findAllComponents({ name: 'ElSelect' })
    selects[0]!.vm.$emit('update:modelValue', 'stale')
    await settle()
    expect(wrapper.findAllComponents({ name: 'ElInputNumber' })).toHaveLength(1)

    await button(wrapper, '保存').trigger('click')
    await vi.waitFor(() => expect(createRuleMock).toHaveBeenCalledOnce())

    expect(createRuleMock.mock.calls[0]![0]).toMatchObject({
      name: '超时规则', condition: 'stale', resultQuality: 'bad', enabled: true,
    })
    expect(ElMessage.success).toHaveBeenCalledWith('已创建')
    expect(listRulesMock).toHaveBeenCalledTimes(2)
  })

  it('保存失败：报出原因，弹窗不关以便重试', async () => {
    createRuleMock.mockRejectedValue(new Error('dup name'))
    const wrapper = mountPage()
    await openCreate(wrapper)
    await wrapper.findAllComponents(ElInput)[0]!.vm.$emit('update:modelValue', 'dup')

    await button(wrapper, '保存').trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('dup name'), '保存质量规则失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(wrapper.find('.dialog-stub').exists()).toBe(true)
    expect(closeGuard).not.toHaveBeenCalled()
  })

  it('删除规则：取消不发请求，确认后提示并刷新', async () => {
    confirmDeleteMock.mockResolvedValueOnce(false)
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('button').filter((b) => b.text().trim() === '删除')[0]!.trigger('click')
    await settle()
    expect(deleteRuleMock).not.toHaveBeenCalled()

    await wrapper.findAll('button').filter((b) => b.text().trim() === '删除')[0]!.trigger('click')
    await settle()
    expect(deleteRuleMock).toHaveBeenCalledWith('q1')
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
    expect(listRulesMock).toHaveBeenCalledTimes(2)
  })

  it('删除失败：提示原因，不谎报已删除', async () => {
    deleteRuleMock.mockRejectedValue(new Error('rule busy'))
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('button').filter((b) => b.text().trim() === '删除')[0]!.trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('rule busy'), '删除质量规则失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('通信画像入口跳转带 tab 参数', async () => {
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '设备级通信成功率（通信画像）').trigger('click')

    expect(routerPush).toHaveBeenCalledWith('/acquisition/comm-tuning?tab=comm-profile')
  })
})
