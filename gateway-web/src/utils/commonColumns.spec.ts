import { describe, it, expect } from 'vitest'
import type { VNode } from 'vue'
import { timeColumn, protocolColumn, addressColumn, tagColumn, actionColumn } from './commonColumns'
import { formatDateTime } from './datetime'

/** 取出 `h(ElTag, props, () => text)` 渲染出的标签属性与文案（slot 可能被包装成函数或对象） */
function slotText(slot: unknown): string {
  if (typeof slot === 'function') return slotText((slot as () => unknown)())
  if (Array.isArray(slot)) return slot.map(slotText).join('')
  if (slot && typeof slot === 'object' && 'default' in slot) {
    const def = (slot as { default: unknown }).default
    return typeof def === 'function' ? slotText((def as () => unknown)()) : ''
  }
  return String(slot ?? '')
}

function tagOf(node: unknown) {
  const vnode = node as VNode
  return { props: vnode.props as Record<string, unknown>, text: slotText(vnode.children) }
}

describe('commonColumns — 跨页面列定义单一来源', () => {
  it('timeColumn 使用统一宽度与 24 小时制格式', () => {
    const col = timeColumn<{ createdAt: number }>('createdAt')
    expect(col.key).toBe('createdAt')
    expect(col.title).toBe('时间')
    expect(col.width).toBe(170)
    expect(col.cell?.({ createdAt: 0 })).toBe(formatDateTime(0))
  })

  it('timeColumn 支持宽度与格式化覆盖', () => {
    const col = timeColumn('ts', { width: 180, format: (v) => (v ? '有值' : '-') })
    expect(col.width).toBe(180)
    expect(col.cell?.({ ts: 0 })).toBe('-')
  })

  it('addressColumn 默认等宽右对齐', () => {
    const col = addressColumn()
    expect(col.key).toBe('address')
    expect(col.mono).toBe(true)
    expect(col.align).toBe('right')
  })

  it('protocolColumn 默认显示中文标签并沿用协议取色', () => {
    const col = protocolColumn<{ protocol: string }>()
    const { text } = tagOf(col.cell?.({ protocol: 'modbus' }))
    expect(text).toBeTruthy()
    expect(text).not.toBe('modbus')
  })

  it('protocolColumn 可保留原始协议名（扫描结果场景）', () => {
    const col = protocolColumn<{ protocol: string }>({ useLabel: false })
    expect(tagOf(col.cell?.({ protocol: 'modbus' })).text).toBe('modbus')
  })

  it('actionColumn 统一「操作」列的 key 与列名', () => {
    const col = actionColumn<{ id: string }>({ cell: () => null })
    expect(col.key).toBe('actions')
    expect(col.title).toBe('操作')
    expect(col.width).toBe(120)
  })

  it('actionColumn 只覆盖差异项，不动列名', () => {
    const col = actionColumn<{ id: string }>({
      key: 'op',
      width: 80,
      align: 'center',
      fixed: 'right',
      cell: (row) => row.id,
    })
    expect(col.key).toBe('op')
    expect(col.title).toBe('操作')
    expect(col.width).toBe(80)
    expect(col.align).toBe('center')
    expect(col.fixed).toBe('right')
    expect(col.cell?.({ id: 'x' })).toBe('x')
  })

  it('tagColumn 的文案与颜色由调用方提供', () => {
    const col = tagColumn<{ ok: boolean }>('ok', {
      title: '结果',
      type: (row) => (row.ok ? 'success' : 'danger'),
      label: (row) => (row.ok ? '成功' : '失败'),
    })
    const ok = tagOf(col.cell?.({ ok: true }))
    const failed = tagOf(col.cell?.({ ok: false }))
    expect(ok.text).toBe('成功')
    expect(failed.text).toBe('失败')
    expect(ok.props.type).toBe('success')
    expect(failed.props.type).toBe('danger')
  })
})
