import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import { defineComponent, h, type VNodeChild } from 'vue'
import type { DeviceEvidence } from '@/utils/deviceEvidence'
import { createDeviceColumns, type DeviceColumnContext, type DeviceRow } from './deviceListColumns'

/**
 * 设备列表列定义的渲染契约。
 *
 * 这些列原本内联在 DeviceList.vue 里，没有独立测试覆盖；拆成模块后
 * 用纯函数 + 显式 context 的形式渲染，得以单测各列的展示规则。
 */

const baseRow: DeviceRow = {
  id: 'd1',
  name: '压力表',
  template: 'tpl1',
  bus: 'bus1',
  protocol: 'modbus',
  address: '1',
  slave_id: null,
  address_map: null,
  interval: 0,
  group: '机房A',
  tags: ['重要'],
}

/** 构造完整的 DeviceEvidence（各字段都有默认值，只覆盖关心的项） */
function makeEvidence(overrides: Partial<DeviceEvidence> = {}): DeviceEvidence {
  return {
    lastGoodTs: 0,
    consecutiveFailures: 0,
    lastFrameHex: '',
    lastFrameDirection: '',
    lastFrameTs: 0,
    lastFrameFailed: false,
    lastFrameError: '',
    ...overrides,
  }
}

function makeCtx(overrides: Partial<DeviceColumnContext> = {}): DeviceColumnContext {
  return {
    commsStats: {},
    commsQualityType: () => 'info',
    commsQualityText: () => '未知',
    backoffQuality: {},
    evidenceByDevice: {},
    onlineStateByDevice: {},
    saveDeviceField: vi.fn(async () => true),
    openAsset: vi.fn(),
    editDevice: vi.fn(),
    removeDevice: vi.fn(),
    gotoPoints: vi.fn(),
    canWriteDevice: false,
    ...overrides,
  }
}

/**
 * 把 cell 返回的 vnode 挂载到真实 DOM 后取文本与 class。
 *
 * 不能直接遍历 vnode.children 取文本：ElTag / ElButton 这类组件的 slot 是
 * 函数（children 是 () => VNode），遍历拿到的只是函数对象。
 */
function renderCell(col: { cell?: (row: DeviceRow) => VNodeChild }, row: DeviceRow) {
  const Host = defineComponent({
    setup() {
      return () => h('div', { class: 'cell-host' }, [col.cell!(row)] as VNodeChild[])
    },
  })
  const wrapper = mount(Host)
  const root = wrapper.element as HTMLElement
  return {
    text: wrapper.text(),
    html: wrapper.html(),
    classes: Array.from(root.querySelectorAll<HTMLElement>('*')).map((e) => e.className),
  }
}

describe('deviceListColumns', () => {
  it('列顺序与拆分前一致（状态 → 通信质量 → 最后成功采集 → 证据 → 基础列 → 操作）', () => {
    const cols = createDeviceColumns(makeCtx())
    expect(cols.map((c) => c.key)).toEqual([
      'online', 'comms', 'last_good_ts', 'evidence', 'id', 'name',
      'template', 'bus', 'protocol', 'address', 'group', 'tags', 'actions',
    ])
  })

  it('状态列：无判定结果时显示「未知」而非默认在线', () => {
    const cols = createDeviceColumns(makeCtx())
    const { text } = renderCell(cols.find((c) => c.key === 'online')!, baseRow)
    expect(text).toContain('未知')
    expect(text).not.toContain('在线')
  })

  it('状态列：在线态显示「在线」并带对应 class', () => {
    const cols = createDeviceColumns(makeCtx({
      onlineStateByDevice: { d1: 'online' },
    }))
    const { text, classes } = renderCell(cols.find((c) => c.key === 'online')!, baseRow)
    expect(text).toContain('在线')
    // 状态点 class 是数组形式（['state-dot', 'online']），逐项匹配
    expect(classes.flatMap((c) => String(c).split(' '))).toContain('online')
  })

  it('通信质量列：本会话有 UI 记录时展示延迟', () => {
    const cols = createDeviceColumns(makeCtx({
      commsStats: { d1: { avgLatency: 12 } },
      commsQualityType: () => 'success',
      commsQualityText: () => '优',
    }))
    const { text } = renderCell(cols.find((c) => c.key === 'comms')!, baseRow)
    expect(text).toContain('12ms')
    expect(text).toContain('优')
  })

  it('通信质量列：avgLatency 为 NaN 时不渲染 "NaNms"', () => {
    const cols = createDeviceColumns(makeCtx({
      commsStats: { d1: { avgLatency: Number.NaN } },
    }))
    const { text } = renderCell(cols.find((c) => c.key === 'comms')!, baseRow)
    expect(text).not.toContain('NaN')
    expect(text).toContain('—')
  })

  it('通信质量列：无 UI 记录时用后端退避质量兜底', () => {
    const cols = createDeviceColumns(makeCtx({
      backoffQuality: { d1: { level: 'warn', text: '波动', intervalLabel: '2.5s/次' } },
    }))
    const { text } = renderCell(cols.find((c) => c.key === 'comms')!, baseRow)
    expect(text).toContain('波动')
    expect(text).toContain('2.5s/次')
  })

  it('最后成功采集列：无数据时显示「从未」', () => {
    const cols = createDeviceColumns(makeCtx())
    const { text } = renderCell(cols.find((c) => c.key === 'last_good_ts')!, baseRow)
    expect(text).toContain('从未')
  })

  it('证据列：无证据时显示「—」', () => {
    const cols = createDeviceColumns(makeCtx())
    const { text } = renderCell(cols.find((c) => c.key === 'evidence')!, baseRow)
    expect(text).toContain('—')
  })

  it('证据列：连续失败次数渲染为标签', () => {
    const cols = createDeviceColumns(makeCtx({
      evidenceByDevice: { d1: makeEvidence({ consecutiveFailures: 5 }) },
    }))
    const { text } = renderCell(cols.find((c) => c.key === 'evidence')!, baseRow)
    expect(text).toContain('连续失败 5')
  })

  it('操作列：无写权限时不渲染编辑/删除', () => {
    const cols = createDeviceColumns(makeCtx({ canWriteDevice: false }))
    const { text } = renderCell(cols.find((c) => c.key === 'actions')!, baseRow)
    expect(text).toContain('点位')
    expect(text).toContain('资产')
    expect(text).not.toContain('编辑')
    expect(text).not.toContain('删除')
  })

  it('操作列：有写权限时渲染编辑/删除', () => {
    const cols = createDeviceColumns(makeCtx({ canWriteDevice: true }))
    const { text } = renderCell(cols.find((c) => c.key === 'actions')!, baseRow)
    expect(text).toContain('编辑')
    expect(text).toContain('删除')
  })
})
