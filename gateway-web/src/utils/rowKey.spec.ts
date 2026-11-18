import { describe, expect, it } from 'vitest'
import { createRowKey } from './rowKey'

/**
 * 契约：行 key 必须与数组下标无关，且不污染数据对象。
 *
 * 用下标当 key 时，删除中间行会让 Vue 复用错误的 DOM，表单里已填的值
 * 错位到隔壁行。本工具用 WeakMap 按对象引用分配稳定 id。
 */
describe('createRowKey', () => {
  it('同一对象永远拿到同一个 key', () => {
    const keyOf = createRowKey()
    const row = { name: 'a' }
    expect(keyOf(row)).toBe(keyOf(row))
  })

  it('不同对象拿到不同 key', () => {
    const keyOf = createRowKey()
    expect(keyOf({ name: 'a' })).not.toBe(keyOf({ name: 'a' }))
  })

  it('删除中间行后，其它行的 key 不变', () => {
    const keyOf = createRowKey()
    const rows = [{ id: 1 }, { id: 2 }, { id: 3 }]
    const before = rows.map((r) => keyOf(r))

    // 删掉中间一行，下标前移
    rows.splice(1, 1)

    // 剩余两行的 key 与删除前完全一致 —— 这是用下标当 key 时会错位的根因
    const after = [keyOf(rows[0]!), keyOf(rows[1]!)]
    expect(after).toEqual([before[0], before[2]])
  })

  it('不往数据对象上写任何字段（后端契约不受影响）', () => {
    const keyOf = createRowKey()
    const row: Record<string, unknown> = { name: 'a' }
    keyOf(row)
    expect(Object.keys(row)).toEqual(['name'])
    expect('_uid' in row).toBe(false)
  })

  it('多个分配器互不干扰', () => {
    const a = createRowKey()
    const b = createRowKey()
    const row = { name: 'a' }
    expect(a(row)).toBe(b(row))
  })
})
