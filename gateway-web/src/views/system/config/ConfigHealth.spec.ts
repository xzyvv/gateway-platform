import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：系统配置 › 配置体检（ConfigHealth）行为测试。
 *
 * 这页的评分是「这份配置能不能上线」的唯一自检结论。历史上空配置能拿 100 分，
 * 等于告诉用户「一切正常」。钉住的核心：
 * - 空配置不给分，明确说「没有可检查的对象」并给出下一步入口；
 * - 评分按错误 15/警告 5/提示 1 扣分，颜色随分数分档；
 * - 模板库拉取失败不阻塞体检（退化为跳过模板类检查）；
 * - 一键修复只对可修复项出现，修复成功后该行不再可修复并重新体检；
 * - 体检失败要可见报错，不能显示「0 个错误 = 健康」。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push: routerPush }) }))
vi.mock('@/api/config', () => ({ getConfig: (...a: unknown[]) => getConfigMock(...a) }))
vi.mock('@/api/templates', () => ({ listTemplates: (...a: unknown[]) => listTemplatesMock(...a) }))
vi.mock('@/utils/configHealth', () => ({ loadConfigIssues: (...a: unknown[]) => loadIssuesMock(...a) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))

const routerPush = vi.hoisted(() => vi.fn())
const getConfigMock = vi.hoisted(() => vi.fn())
const listTemplatesMock = vi.hoisted(() => vi.fn())
const loadIssuesMock = vi.hoisted(() => vi.fn())
const showToast = vi.hoisted(() => vi.fn())

import ConfigHealth from './ConfigHealth.vue'
import type { ConfigIssue } from '@/utils/configHealth'

const CFG = { buses: [{ id: 'bus1' }], devices: [{ id: 'dev1' }], points: [{ id: 'p1' }] }

const ISSUES: ConfigIssue[] = [
  {
    id: 'i1', level: 'error', category: '总线', target: 'bus1',
    description: '总线不存在', suggestion: '创建总线', autoFixable: false,
  },
  {
    id: 'i2', level: 'warning', category: '点位', target: 'p1',
    description: '地址重复', suggestion: '调整地址', autoFixable: true,
    fix: () => fixMock(),
  },
  {
    id: 'i3', level: 'info', category: '设备', target: 'dev1',
    description: '建议补全型号', suggestion: '填写型号', autoFixable: false,
  },
]
const fixMock = vi.hoisted(() => vi.fn())

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(ConfigHealth)
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

async function runCheck(wrapper: ReturnType<typeof mountPage>, text = '开始检查') {
  await button(wrapper, text).trigger('click')
  await vi.waitFor(() => expect(wrapper.find('.summary-text h3').text()).not.toBe('配置健康检查'))
  await settle()
}

describe('ConfigHealth 配置体检', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    getConfigMock.mockResolvedValue(CFG)
    listTemplatesMock.mockResolvedValue([{ name: 'tpl-a' }])
    loadIssuesMock.mockResolvedValue(ISSUES.map((i) => ({ ...i })))
    fixMock.mockResolvedValue(undefined)
  })

  it('首次进入未检查：显示「开始检查」且不渲染问题清单', async () => {
    const wrapper = mountPage()
    await settle()

    expect(wrapper.find('.summary-text h3').text()).toBe('配置健康检查')
    expect(wrapper.text()).toContain('检查配置中的矛盾项与高风险写法')
    expect(wrapper.findAllComponents({ name: 'DataTable' })).toHaveLength(0)
    expect(button(wrapper, '开始检查').exists()).toBe(true)
  })

  it('空配置不给满分：明确说无法体检并给出下一步入口', async () => {
    getConfigMock.mockResolvedValue({ buses: [], devices: [], points: [] })
    loadIssuesMock.mockResolvedValue([])
    const wrapper = mountPage()
    await button(wrapper, '开始检查').trigger('click')
    await vi.waitFor(() => expect(ElMessage.warning).toHaveBeenCalled())
    await settle()

    expect(ElMessage.warning).toHaveBeenCalledWith('尚未配置任何内容，无法体检')
    expect(wrapper.text()).toContain('尚未配置任何内容，无法体检')
    expect(wrapper.findAllComponents({ name: 'ElProgress' })).toHaveLength(0)
    expect(wrapper.findAllComponents({ name: 'DataTable' })).toHaveLength(0)

    await button(wrapper, '去新增总线').trigger('click')
    expect(routerPush).toHaveBeenCalledWith({ path: '/system/config', query: { tab: 'runtime-config' } })
    await button(wrapper, '去配置点位').trigger('click')
    expect(routerPush).toHaveBeenCalledWith({ path: '/points', query: { tab: 'points' } })
  })

  it('有内容时按 15/5/1 扣分，并逐项列出问题与修复建议', async () => {
    const wrapper = mountPage()
    await runCheck(wrapper)

    expect(wrapper.find('.summary-text h3').text()).toBe('配置健康评分 79 分')
    expect(wrapper.text()).toContain('1 个错误')
    expect(wrapper.text()).toContain('1 个警告')
    expect(wrapper.text()).toContain('1 个提示')
    expect(wrapper.findComponent({ name: 'ElProgress' }).props('percentage')).toBe(79)
    expect(wrapper.findComponent({ name: 'ElProgress' }).props('color')).toBe('var(--el-color-warning)')

    const table = wrapper.findComponent({ name: 'DataTable' })
    expect((table.props() as unknown as { data: ConfigIssue[] }).data).toHaveLength(3)
    expect(wrapper.text()).toContain('问题清单（共 3 项）')
    expect(wrapper.text()).toContain('总线不存在')
    expect(wrapper.text()).toContain('需手动处理')
    expect(ElMessage.success).toHaveBeenCalledWith('检查完成')
  })

  it('分数低于 50 用危险色，80 分及以上用成功色', async () => {
    loadIssuesMock.mockResolvedValue([
      { id: 'a', level: 'error', category: 'x', target: 't', description: 'd', suggestion: 's', autoFixable: false },
      { id: 'b', level: 'error', category: 'x', target: 't', description: 'd', suggestion: 's', autoFixable: false },
      { id: 'c', level: 'error', category: 'x', target: 't', description: 'd', suggestion: 's', autoFixable: false },
      { id: 'd', level: 'error', category: 'x', target: 't', description: 'd', suggestion: 's', autoFixable: false },
    ])
    const wrapper = mountPage()
    await runCheck(wrapper)
    expect(wrapper.findComponent({ name: 'ElProgress' }).props('color')).toBe('var(--el-color-danger)')

    loadIssuesMock.mockResolvedValue([
      { id: 'w', level: 'warning', category: 'x', target: 't', description: 'd', suggestion: 's', autoFixable: false },
    ])
    const wrapper2 = mountPage()
    await runCheck(wrapper2)
    expect(wrapper2.findComponent({ name: 'ElProgress' }).props('percentage')).toBe(95)
    expect(wrapper2.findComponent({ name: 'ElProgress' }).props('color')).toBe('var(--el-color-success)')
  })

  it('模板库拉取失败不阻塞体检，只是跳过模板类检查', async () => {
    listTemplatesMock.mockRejectedValue(new Error('tpl 500'))
    const wrapper = mountPage()
    await runCheck(wrapper)

    expect(loadIssuesMock).toHaveBeenCalledWith(CFG, new Set())
    expect(ElMessage.success).toHaveBeenCalledWith('检查完成')
    expect(showToast).not.toHaveBeenCalled()
  })

  it('体检失败：报出原因且不显示评分（避免 0 错误=健康的假象）', async () => {
    getConfigMock.mockRejectedValue(new Error('cfg 500'))
    const wrapper = mountPage()
    await button(wrapper, '开始检查').trigger('click')
    await vi.waitFor(() => expect(showToast).toHaveBeenCalled())

    expect(showToast).toHaveBeenCalledWith(new Error('cfg 500'), '检查失败')
    expect(wrapper.find('.summary-text h3').text()).toBe('配置健康检查')
    expect(wrapper.findAllComponents({ name: 'DataTable' })).toHaveLength(0)
  })

  it('一键修复：只对可修复项出现，修复成功后重新体检', async () => {
    const wrapper = mountPage()
    await runCheck(wrapper)

    const fixBtns = wrapper.findAll('button').filter((b) => b.text().trim() === '一键修复')
    expect(fixBtns).toHaveLength(1)

    await fixBtns[0]!.trigger('click')
    await settle()

    expect(fixMock).toHaveBeenCalledOnce()
    expect(ElMessage.success).toHaveBeenCalledWith('已自动修复：地址重复')
    expect(loadIssuesMock).toHaveBeenCalledTimes(2)
  })

  it('修复失败：可见报错，修复入口保留', async () => {
    fixMock.mockRejectedValue(new Error('写配置失败'))
    const wrapper = mountPage()
    await runCheck(wrapper)

    await wrapper.findAll('button').filter((b) => b.text().trim() === '一键修复')[0]!.trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('写配置失败'), '修复失败')
    expect(wrapper.findAll('button').filter((b) => b.text().trim() === '一键修复')).toHaveLength(1)
  })
})
