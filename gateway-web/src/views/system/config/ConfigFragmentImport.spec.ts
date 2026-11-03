import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：参数配置 › 片段导入（ConfigFragmentImport）行为测试。
 *
 * 这一页是现场「只贴一段配置」的入口：按主键把 6 类列表节合并进当前配置。
 * 钉住的核心：
 * - 坏输入（空 / 非法 JSON / 不支持的配置节）在本地就被拦住，绝不发给后端；
 * - 校验结论（新增/覆盖条数、后端错误数、模板模式拦截）直接决定「确认导入」是否可点；
 * - 只有确认导入后才真正写配置，失败原因要留在页面上而不是一闪而过的 toast。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))
vi.mock('@/api/config', () => ({
  getConfig: vi.fn(),
  postConfig: vi.fn(),
  validateConfigDraft: vi.fn(),
}))

const push = vi.hoisted(() => vi.fn())

import ConfigFragmentImport from './ConfigFragmentImport.vue'
import DataTable from '@/components/DataTable.vue'
import ImportOutcome from '@/components/ImportOutcome.vue'
import { getConfig, postConfig, validateConfigDraft } from '@/api/config'

const CURRENT = {
  buses: [{ port_name: 'com1', protocol: 'modbus' }],
  devices: [],
  points: [],
  device_groups: [],
  virtual_points: [],
  convert_rules: [],
}

const NEW_POINTS = {
  points: [
    { id: 'p_new', name: '新点位A', bus: 'com1', protocol: 'modbus', address: '40001' },
    { id: 'p_new2', name: '新点位B', bus: 'com1', protocol: 'modbus', address: '40002' },
  ],
}

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(ConfigFragmentImport)
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

async function typeFragment(wrapper: ReturnType<typeof mountPage>, text: string) {
  await wrapper.find('textarea[aria-label="待导入的配置片段"]').setValue(text)
}

describe('ConfigFragmentImport 片段导入', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(getConfig).mockResolvedValue(structuredClone(CURRENT) as never)
    vi.mocked(validateConfigDraft).mockResolvedValue({ issues: [], errors: 0, warnings: 0 } as never)
    vi.mocked(postConfig).mockResolvedValue(undefined as never)
  })

  it('空片段直接拦下：不发任何请求，并给出可执行提示', async () => {
    const wrapper = mountPage()
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('请粘贴或上传要导入的配置片段')
    expect(getConfig).not.toHaveBeenCalled()
    expect(validateConfigDraft).not.toHaveBeenCalled()
  })

  it('非法 JSON 在本地解析失败，绝不发请求', async () => {
    const wrapper = mountPage()
    await typeFragment(wrapper, '{ not json ')
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(vi.mocked(ElMessage.error).mock.calls[0]![0]).toContain('JSON 解析失败：')
    expect(getConfig).not.toHaveBeenCalled()
    expect(wrapper.findAllComponents(DataTable)).toHaveLength(0)
  })

  it('包含不支持导入的配置节时，明确指出该节并引导改用整份导入', async () => {
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify({ mqtt: { host: 'x' } }))
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(vi.mocked(ElMessage.error).mock.calls[0]![0]).toBe(
      '不支持片段导入的配置节：mqtt；这些节请用「导入导出」页的整份配置导入',
    )
    expect(getConfig).not.toHaveBeenCalled()
  })

  it('合法片段：合并后交后端校验，并展示各节新增/覆盖统计', async () => {
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(getConfig).toHaveBeenCalledOnce()
    // 合并结果：原有总线保留，点位追加 2 条
    const merged = vi.mocked(validateConfigDraft).mock.calls[0]![0] as unknown as {
      buses: unknown
      points: Array<{ id: string }>
    }
    expect(merged.buses).toEqual(CURRENT.buses)
    expect(merged.points.map((p) => p.id)).toEqual(['p_new', 'p_new2'])

    const stats = (
      wrapper.findAllComponents(DataTable)[0]!.props() as unknown as { data: Array<Record<string, unknown>> }
    ).data
    expect(stats).toHaveLength(1)
    expect(stats[0]).toMatchObject({ label: '点位', added: 2, updated: 0, unchanged: 0 })
    expect(wrapper.find('.cfi-head .el-tag').text()).toBe('可导入：新增 / 覆盖 2 条')
    expect(button(wrapper, '确认导入').attributes('disabled')).toBeUndefined()
  })

  it('片段与当前配置完全一致时不制造变更，确认导入不可点', async () => {
    const wrapper = mountPage()
    await typeFragment(
      wrapper,
      JSON.stringify({ buses: [{ port_name: 'com1', protocol: 'modbus' }] }),
    )
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(wrapper.find('.cfi-head .el-tag').text()).toBe('与当前配置完全一致')
    expect(button(wrapper, '确认导入').attributes('disabled')).toBeDefined()
  })

  it('后端校验有错误时展示错误条数并禁止导入', async () => {
    vi.mocked(validateConfigDraft).mockResolvedValue({
      issues: [
        { level: 'error', field: 'points[0].id', message: '点位 ID 重复' },
        { level: 'error', field: 'points[1].bus', message: '总线不存在' },
        { level: 'warning', field: 'points[0].interval', message: '采集周期过短' },
      ],
      errors: 2,
      warnings: 1,
    } as never)
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(wrapper.find('.cfi-head .el-tag').text()).toBe('存在 2 处错误')
    expect(wrapper.text()).toContain('后端校验：2 处错误 / 1 处警告')
    const issues = (
      wrapper.findAllComponents(DataTable)[1]!.props() as unknown as { data: unknown[] }
    ).data
    expect(issues).toHaveLength(3)
    expect(button(wrapper, '确认导入').attributes('disabled')).toBeDefined()
  })

  it('模板模式下只导点位会提示「不参与采集」，并禁止导入', async () => {
    vi.mocked(getConfig).mockResolvedValue({ ...structuredClone(CURRENT), devices: [{ id: 'dev-1' }] } as never)
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(wrapper.text()).toContain('当前网关已配置设备模板，新增点位不会参与采集')
    expect(button(wrapper, '确认导入').attributes('disabled')).toBeDefined()
  })

  it('校验接口失败：如实报错并清空上一次的校验结果', async () => {
    vi.mocked(validateConfigDraft).mockRejectedValue(new Error('validate 500'))
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('校验失败：validate 500')
    expect(wrapper.findAllComponents(DataTable)).toHaveLength(0)
  })

  it('二次校验复用已拉取的当前配置，不重复请求整份配置', async () => {
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()
    await button(wrapper, '开始校验').trigger('click')
    await settle()

    expect(getConfig).toHaveBeenCalledOnce()
    expect(validateConfigDraft).toHaveBeenCalledTimes(2)
  })

  it('确认导入才写配置：成功后面板给出新增/覆盖条数并清空输入', async () => {
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    expect(postConfig).toHaveBeenCalledOnce()
    expect(vi.mocked(postConfig).mock.calls[0]![0]).toMatchObject({ points: [{ id: 'p_new' }, { id: 'p_new2' }] })
    const outcome = wrapper.findComponent(ImportOutcome)
    expect(outcome.props('success')).toBe(true)
    expect(outcome.props('subtitle')).toContain('新增 2 条、覆盖 0 条')
    expect((wrapper.find('textarea[aria-label="待导入的配置片段"]').element as HTMLTextAreaElement).value).toBe('')

    // 结果面板的后续动作：查看配置版本 / 继续导入
    await button(wrapper, '继续导入').trigger('click')
    await settle()
    expect(wrapper.findComponent(ImportOutcome).exists()).toBe(false)
  })

  it('提交失败时把后端原因留在结果面板，不谎报成功', async () => {
    vi.mocked(postConfig).mockRejectedValue(new Error('配置校验未通过：总线 com1 不存在'))
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    const outcome = wrapper.findComponent(ImportOutcome)
    expect(outcome.props('success')).toBe(false)
    expect(outcome.props('error')).toBe('配置校验未通过：总线 com1 不存在')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('「填入示例」给出可直接校验的点位片段', async () => {
    const wrapper = mountPage()
    await button(wrapper, '填入示例').trigger('click')
    await settle()

    const value = (wrapper.find('textarea[aria-label="待导入的配置片段"]').element as HTMLTextAreaElement).value
    expect(value).toContain('p_temp')
    expect(value).toContain('"points"')
  })

  it('结果面板可跳转配置版本页，便于导入后回滚', async () => {
    const wrapper = mountPage()
    await typeFragment(wrapper, JSON.stringify(NEW_POINTS))
    await button(wrapper, '开始校验').trigger('click')
    await settle()
    await button(wrapper, '确认导入').trigger('click')
    await settle()

    await button(wrapper, '查看配置版本').trigger('click')
    expect(push).toHaveBeenCalledWith({ path: '/system/config', query: { tab: 'config-versions' } })
  })
})
