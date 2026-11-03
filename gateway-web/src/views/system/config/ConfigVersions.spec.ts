import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：系统配置 › 版本历史（ConfigVersions）行为测试。
 *
 * 回滚会直接覆盖现场正在运行的配置，是这套 UI 里破坏力最大的按钮之一。
 * 钉住的核心：
 * - 回滚/撤销到上一版本都必须先确认，取消就一个请求都不发；
 * - 「撤销到上一版本」在 0/1 个版本、无历史版本时不能假装能撤销；
 * - 版本对比要真的把差异字段翻译成中文，无差异不能显示成有差异；
 * - 快照/对比加载失败要可见报错，不能用空白冒充「没有内容」；
 * - 备份、回滚成功后重新拉列表，用户看到的版本号是后端的真实结果。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('@/api/config', () => ({
  listConfigVersions: (...a: unknown[]) => listVersionsMock(...a),
  getConfigVersion: (...a: unknown[]) => getVersionMock(...a),
  rollbackConfigVersion: (...a: unknown[]) => rollbackMock(...a),
  backupConfig: (...a: unknown[]) => backupMock(...a),
  compareConfigVersions: (...a: unknown[]) => compareMock(...a),
}))
vi.mock('@/utils/confirm', () => ({ confirmAction: (...a: unknown[]) => confirmActionMock(...a) }))
vi.mock('@/utils/logger', () => ({ logError: (...a: unknown[]) => logErrorMock(...a) }))

const listVersionsMock = vi.hoisted(() => vi.fn())
const getVersionMock = vi.hoisted(() => vi.fn())
const rollbackMock = vi.hoisted(() => vi.fn())
const backupMock = vi.hoisted(() => vi.fn())
const compareMock = vi.hoisted(() => vi.fn())
const confirmActionMock = vi.hoisted(() => vi.fn())
const logErrorMock = vi.hoisted(() => vi.fn())

import ConfigVersions from './ConfigVersions.vue'
import type { ConfigVersionSummary } from '@/api/config'

const DialogStub = {
  name: 'ElDialog',
  props: ['modelValue', 'title', 'width', 'top'],
  template: '<div class="dialog-stub"><slot /><slot name="footer" /></div>',
}

const VERSIONS: ConfigVersionSummary[] = [
  { id: 3, created_at: 1_760_000_000, message: '当前版本', size: 2 * 1024 * 1024, is_current: true },
  { id: 2, created_at: 1_759_990_000, message: '', size: 1536, is_current: false },
  { id: 1, created_at: 0, message: '初始', size: 120, is_current: false },
]

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(ConfigVersions, { global: { stubs: { ElDialog: DialogStub } } })
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

describe('ConfigVersions 配置版本历史', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listVersionsMock.mockResolvedValue([...VERSIONS])
    getVersionMock.mockResolvedValue({ buses: [{ id: 'bus1' }] })
    rollbackMock.mockResolvedValue({ status: 'ok' })
    backupMock.mockResolvedValue({ version_id: 4, points: 12, devices: 3 })
    compareMock.mockResolvedValue({
      from: { id: 1, created_at: 0 },
      to: { id: 3, created_at: 1_760_000_000 },
      has_changes: true,
      changed_fields: ['buses', 'mqtt', 'unknown_key'],
    })
    confirmActionMock.mockResolvedValue(true)
  })

  it('无版本时显示空态并提供「刷新一下」入口', async () => {
    listVersionsMock.mockResolvedValue([])
    const wrapper = mountPage()
    await settle()

    expect(wrapper.text()).toContain('保存配置时会自动产生版本快照，方便随时回滚')
    expect(wrapper.findAll('.version-card')).toHaveLength(0)
    expect(button(wrapper, '撤销到上一版本').attributes('disabled')).toBeDefined()

    await button(wrapper, '刷新一下').trigger('click')
    await settle()
    expect(listVersionsMock).toHaveBeenCalledTimes(2)
  })

  it('列表渲染：当前版本有标记，空备注有占位，体积/时间按可读单位展示', async () => {
    const wrapper = mountPage()
    await settle()

    expect(wrapper.text()).toContain('共 3 个版本')
    expect(wrapper.findAll('.version-card')).toHaveLength(3)
    expect(wrapper.findAll('.version-title .el-tag').map((t) => t.text())).toEqual([
      'v3', '当前', 'v2', 'v1',
    ])
    expect(wrapper.findAll('.remark').map((n) => n.text())).toEqual(['当前版本', '(无备注)', '初始'])
    expect(wrapper.findAll('.version-meta').map((n) => n.text())).toEqual([
      expect.stringContaining('2.00 MB'),
      expect.stringContaining('1.5 KB'),
      expect.stringContaining('120 B'),
    ])
    // created_at = 0 时不能显示成 1970 年
    expect(wrapper.findAll('.version-meta')[2]!.text()).toContain('-')
    // 当前版本不能回滚/对比
    const ops = wrapper.findAll('.version-ops')[0]!.findAll('button')
    expect(ops[1]!.attributes('disabled')).toBeDefined()
    expect(ops[2]!.attributes('disabled')).toBeDefined()
  })

  it('加载失败：可见报错并留日志，不显示伪造的版本', async () => {
    listVersionsMock.mockRejectedValue(new Error('cfg 500'))
    const wrapper = mountPage()
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('加载版本列表失败：cfg 500')
    expect(logErrorMock).toHaveBeenCalledWith('cfg-ver', expect.any(Error))
    expect(wrapper.findAll('.version-card')).toHaveLength(0)
  })

  it('手动备份：成功后提示版本号/点位/设备并重新拉列表', async () => {
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '立即备份').trigger('click')
    await settle()

    expect(backupMock).toHaveBeenCalledOnce()
    expect(ElMessage.success).toHaveBeenCalledWith('已创建备份 v4（12 点位 / 3 设备）')
    expect(listVersionsMock).toHaveBeenCalledTimes(2)
  })

  it('手动备份失败：报出可纠正原因，不谎报成功', async () => {
    backupMock.mockRejectedValue(new Error('磁盘只读'))
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '立即备份').trigger('click')
    await settle()

    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(ElMessage.error).toHaveBeenCalledWith('手动备份失败：磁盘只读')
  })

  it('回滚必须先确认：取消则一个写请求都不发', async () => {
    confirmActionMock.mockResolvedValue(false)
    const wrapper = mountPage()
    await settle()

    await wrapper.findAll('.version-ops')[1]!.findAll('button')[2]!.trigger('click')
    await settle()

    expect(confirmActionMock).toHaveBeenCalledWith(
      expect.objectContaining({ title: '回滚确认', message: expect.stringContaining('v2') }),
    )
    expect(rollbackMock).not.toHaveBeenCalled()
  })

  it('确认回滚：按版本号下发、提示并刷新列表', async () => {
    const wrapper = mountPage()
    await settle()

    await wrapper.findAll('.version-ops')[1]!.findAll('button')[2]!.trigger('click')
    await settle()

    expect(rollbackMock).toHaveBeenCalledWith(2)
    expect(ElMessage.success).toHaveBeenCalledWith('已回滚到 v2')
    expect(listVersionsMock).toHaveBeenCalledTimes(2)
  })

  it('回滚失败：报错并保留原列表，不让用户以为已切换', async () => {
    rollbackMock.mockRejectedValue(new Error('版本已损坏'))
    const wrapper = mountPage()
    await settle()

    await wrapper.findAll('.version-ops')[1]!.findAll('button')[2]!.trigger('click')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('回滚失败：版本已损坏')
    expect(wrapper.findAll('.version-card')).toHaveLength(3)
  })

  it('撤销到上一版本：只有一个版本时按钮禁用，无历史版本时给出提示', async () => {
    listVersionsMock.mockResolvedValue([
      { id: 2, created_at: 1_760_000_000, message: 'a', size: 1, is_current: true },
      { id: 1, created_at: 1_750_000_000, message: 'b', size: 1, is_current: false },
    ])
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '撤销到上一版本').trigger('click')
    await settle()
    expect(rollbackMock).toHaveBeenCalledWith(1)

    // 全部都是 current 时没有可撤销项，不能静默无反应
    listVersionsMock.mockResolvedValue([
      { id: 2, created_at: 1_760_000_000, message: 'a', size: 1, is_current: true },
      { id: 1, created_at: 1_750_000_000, message: 'b', size: 1, is_current: true },
    ])
    const wrapper2 = mountPage()
    await settle()
    await button(wrapper2, '撤销到上一版本').trigger('click')
    await settle()
    expect(ElMessage.warning).toHaveBeenCalledWith('没有可撤销的历史版本')
    expect(rollbackMock).toHaveBeenCalledTimes(1)
  })

  it('查看快照：加载中用代码行骨架，成功后展示 JSON', async () => {
    let resolveSnap!: (v: unknown) => void
    getVersionMock.mockReturnValue(new Promise((r) => { resolveSnap = r }))
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '查看快照').trigger('click')
    await nextTick()
    expect(wrapper.find('.snapshot-skeleton').exists()).toBe(true)

    resolveSnap({ buses: [{ id: 'bus1' }], points: [] })
    await settle()
    expect(wrapper.find('.snapshot-skeleton').exists()).toBe(false)
    expect(wrapper.find('.snapshot-body pre').text()).toContain('"bus1"')
  })

  it('快照加载失败：报错且明确是「暂无快照内容」，不显示空白', async () => {
    getVersionMock.mockRejectedValue(new Error('snap 404'))
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '查看快照').trigger('click')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith('加载快照失败：snap 404')
    expect(wrapper.find('.snapshot-body').text()).toContain('暂无快照内容')
  })

  it('版本不足 2 个时不能打开对比', async () => {
    listVersionsMock.mockResolvedValue([VERSIONS[0]!])
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '版本对比').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('至少需要 2 个版本才能对比')
  })

  it('版本对比：差异字段翻译成中文，未知字段原样保留', async () => {
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '版本对比').trigger('click')
    await settle()
    await button(wrapper, '对比').trigger('click')
    await settle()

    expect(compareMock).toHaveBeenCalledWith(1, 3)
    const result = wrapper.find('.compare-result')
    expect(result.text()).toContain('3 项差异')
    expect(result.findAll('.changed-fields .el-tag').map((t) => t.text())).toEqual([
      '总线配置', 'MQTT 通道', 'unknown_key',
    ])
    expect(result.findAll('.pane-body')[0]!.text()).toContain('"bus1"')
  })

  it('对比同版本或未选版本时拦下，不发请求', async () => {
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '版本对比').trigger('click')
    await settle()
    const selects = wrapper.findAllComponents({ name: 'ElSelect' })
    selects[1]!.vm.$emit('update:modelValue', 1)
    await settle()
    await button(wrapper, '对比').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请选择两个不同的版本')
    expect(compareMock).not.toHaveBeenCalled()
  })

  it('无差异时明确显示「无差异」且不渲染差异字段区', async () => {
    compareMock.mockResolvedValue({
      from: { id: 1, created_at: 0 },
      to: { id: 3, created_at: 1_760_000_000 },
      has_changes: false,
      changed_fields: [],
    })
    const wrapper = mountPage()
    await settle()

    await button(wrapper, '版本对比').trigger('click')
    await settle()
    await button(wrapper, '对比').trigger('click')
    await settle()

    expect(wrapper.find('.compare-result').text()).toContain('无差异')
    expect(wrapper.find('.changed-fields').exists()).toBe(false)
  })

  it('与当前版本对比：点历史版本的「与当前对比」会自动跑一次对比', async () => {
    const wrapper = mountPage()
    await settle()

    await wrapper.findAll('.version-ops')[1]!.findAll('button')[1]!.trigger('click')
    await settle()
    expect(compareMock).toHaveBeenCalledWith(2, 3)
    expect(wrapper.find('.compare-result').exists()).toBe(true)
  })
})
