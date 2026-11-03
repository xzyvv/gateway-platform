import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage, ElProgress, ElSwitch } from 'element-plus'

/**
 * D1：系统维护 › 蜂窝网络（CellularStatus）行为测试。
 *
 * 这页的结论决定运维是否认为「4G 可用」。最危险的几类误导：
 * - 把 CSQ=99（AT 指令定义的无信号/未知）画成满格；
 * - 非 Linux 平台明明不拨号，却让用户以为蜂窝已可上网；
 * - 加载失败后静默空页；
 * - 后端英文枚举直接抛给用户。
 * 这里逐条钉住。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('@/api/network', () => ({ getCellularStatus: getCellularStatusMock }))
vi.mock('@/composables/core/usePolling', () => ({
  usePolling: (..._a: unknown[]) => ({ start: startPoll, stop: stopPoll }),
}))
vi.mock('@/utils/logger', () => ({ logError: logErrorMock }))

const getCellularStatusMock = vi.hoisted(() => vi.fn())
const startPoll = vi.hoisted(() => vi.fn())
const stopPoll = vi.hoisted(() => vi.fn())
const logErrorMock = vi.hoisted(() => vi.fn())

import CellularStatus from './CellularStatus.vue'

type Status = {
  enabled: boolean
  dial_supported: boolean
  platform: string
  status: {
    state: string
    signal_strength: number
    operator: string
    ip_address: string | null
    network_type: string
    updated_at: number
  } | null
  link_state: string | null
}

function makeStatus(patch: Partial<Status> = {}, patchInner: Record<string, unknown> = {}): Status {
  return {
    enabled: true,
    dial_supported: true,
    platform: 'linux',
    status: {
      state: 'Connected',
      signal_strength: 25,
      operator: '中国移动',
      ip_address: '10.0.0.8',
      network_type: 'LTE',
      updated_at: 1_760_000_000,
      ...patchInner,
    },
    link_state: 'EthernetUp',
    ...patch,
  }
}

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(CellularStatus)
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

describe('CellularStatus 蜂窝网络', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    getCellularStatusMock.mockResolvedValue(makeStatus())
  })

  it('未启用：明确显示「蜂窝模块未启用」，头标签写「配置未启用」', async () => {
    getCellularStatusMock.mockResolvedValue(makeStatus({ enabled: false, status: null, link_state: null }))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.text()).toContain('蜂窝模块未启用')
    expect(wrapper.find('.count-badge').text()).toBe('配置未启用')
    expect(wrapper.findComponent({ name: 'ElTag' }).props('type')).toBe('info')
    expect(wrapper.find('.platform-alert').exists()).toBe(false)
  })

  it('首屏加载中先渲染骨架（6 张信息卡 + 链路 + 信号），数据到达后替换', async () => {
    let resolve!: (v: Status) => void
    getCellularStatusMock.mockReturnValue(new Promise<Status>((r) => { resolve = r }))
    const wrapper = mountPage()
    await nextTick()

    expect(wrapper.findAllComponents({ name: 'ElSkeleton' })).toHaveLength(8)
    expect(wrapper.findAll('.info-label')).toHaveLength(0)

    resolve(makeStatus())
    await settle()
    expect(wrapper.findAllComponents({ name: 'ElSkeleton' })).toHaveLength(0)
    expect(wrapper.findAll('.info-label')).toHaveLength(6)
  })

  it('非 Linux 平台且配置启用：提示「不支持 4G 拨号」并说明只做链路监控', async () => {
    getCellularStatusMock.mockResolvedValue(
      makeStatus({ platform: 'windows', dial_supported: false }),
    )
    const wrapper = mountPage()
    await settle()

    const alert = wrapper.find('.platform-alert')
    expect(alert.exists()).toBe(true)
    expect(alert.text()).toContain('当前平台不支持 4G 拨号')
    expect(alert.text()).toContain('windows')
    expect(alert.text()).toContain('不会发起 PPP 拨号')
  })

  it('启用但后端还没采到状态：显示「暂无状态数据」，不渲染空白卡片', async () => {
    getCellularStatusMock.mockResolvedValue(makeStatus({ status: null, link_state: null }))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.text()).toContain('蜂窝模块已启用，暂无状态数据')
    expect(wrapper.findAll('.info-card')).toHaveLength(0)
    expect(wrapper.find('.count-badge').text()).toBe('配置已启用')
  })

  it('正常状态：英文枚举本地化，6 张卡显示真实值，信号 25/31 映射为 81% 绿色', async () => {
    const wrapper = mountPage()
    await settle()

    const labels = wrapper.findAll('.info-label').map((n) => n.text())
    expect(labels).toEqual(['连接状态', '运营商', 'IP 地址', '网络类型', '信号强度', '更新时间'])
    const values = wrapper.findAll('.info-value').map((n) => n.text())
    expect(values).toContain('已连接')
    expect(values).toContain('中国移动')
    expect(values).toContain('10.0.0.8')
    expect(values).toContain('LTE')
    expect(values).toContain('CSQ 25')

    const progress = wrapper.findComponent(ElProgress)
    expect(progress.props('percentage')).toBe(81)
    expect(progress.props('color')).toBe('var(--text-success)')
    expect(wrapper.find('.signal-value').text()).toBe('25 / 31')
  })

  it('CSQ=99（未知）不画成满格：显示「未知」、进度 0、红色', async () => {
    getCellularStatusMock.mockResolvedValue(makeStatus({}, { signal_strength: 99 }))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.find('.signal-value').text()).toBe('未知')
    expect(wrapper.findAll('.info-value').map((n) => n.text())).toContain('未知')
    const progress = wrapper.findComponent(ElProgress)
    expect(progress.props('percentage')).toBe(0)
    expect(progress.props('color')).toBe('var(--text-danger)')
  })

  it('链路状态：蜂窝上行=警告色，断开=危险色，以太网=成功色，未知值原样展示', async () => {
    const cases: Array<[string, string, string]> = [
      ['CellularUp', '蜂窝上行', 'el-tag--warning'],
      ['Down', '链路断开', 'el-tag--danger'],
      ['EthernetUp', '以太网上行', 'el-tag--success'],
      ['Weird', 'Weird', 'el-tag--success'],
    ]
    for (const [state, text, cls] of cases) {
      getCellularStatusMock.mockResolvedValue(makeStatus({ link_state: state }))
      const wrapper = mountPage()
      await settle()

      const tag = wrapper.find('.link-card .el-tag')
      expect(tag.text()).toBe(text)
      expect(tag.classes()).toContain(cls)
      wrapper.unmount()
    }
  })

  it('未知状态枚举原样展示，不吞掉后端新增值', async () => {
    getCellularStatusMock.mockResolvedValue(makeStatus({}, { state: 'SimNotReady' }))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.findAll('.info-value')[0]!.text()).toBe('SIM 未就绪')
  })

  it('加载失败：给出带原因的报错并写日志，页面不崩', async () => {
    getCellularStatusMock.mockRejectedValue(new Error('upstream 502'))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.exists()).toBe(true)
    expect(ElMessage.error).toHaveBeenCalledWith('加载蜂窝网络状态失败：upstream 502')
    expect(logErrorMock).toHaveBeenCalledWith('cell', expect.any(Error))
  })

  it('刷新按钮重新拉取并更新时间戳', async () => {
    const wrapper = mountPage()
    await settle()
    expect(getCellularStatusMock).toHaveBeenCalledTimes(1)
    expect(wrapper.find('.last-update').text()).toContain('最后更新')

    getCellularStatusMock.mockResolvedValue(makeStatus({}, { state: 'Dialing' }))
    await button(wrapper, '刷新').trigger('click')
    await settle()

    expect(getCellularStatusMock).toHaveBeenCalledTimes(2)
    expect(wrapper.findAll('.info-value')[0]!.text()).toBe('拨号中')
  })

  it('自动刷新开关：开启立刻拉一次，关闭停止轮询', async () => {
    const wrapper = mountPage()
    await settle()

    const sw = wrapper.findComponent(ElSwitch)
    sw.vm.$emit('change', true)
    expect(startPoll).toHaveBeenCalledWith(true)

    sw.vm.$emit('change', false)
    expect(stopPoll).toHaveBeenCalledOnce()
  })
})
