import { describe, it, expect } from 'vitest'
import { mount } from '@vue/test-utils'
import TableSkeleton from './TableSkeleton.vue'

describe('TableSkeleton', () => {
  it('按行/列数渲染表格占位，并声明无障碍加载状态', () => {
    const wrapper = mount(TableSkeleton, { props: { rows: 5, columns: 3 } })

    // 表头 1 行 + 数据 5 行
    expect(wrapper.findAll('.sk-row')).toHaveLength(6)
    expect(wrapper.findAll('.sk-cell')).toHaveLength(18)
    // 屏幕阅读器要能知道这是"加载中"，而不是一片空白表格
    const root = wrapper.get('.table-skeleton')
    expect(root.attributes('role')).toBe('status')
    expect(root.attributes('aria-busy')).toBe('true')
    expect(root.attributes('aria-label')).toBe('正在加载数据')
  })

  it('紧凑模式给所有数据行加 dense 类', () => {
    const wrapper = mount(TableSkeleton, { props: { rows: 3, dense: true } })
    // 表头不参与紧凑，只有数据行加
    expect(wrapper.findAll('.sk-row.sk-dense')).toHaveLength(3)
  })
})
