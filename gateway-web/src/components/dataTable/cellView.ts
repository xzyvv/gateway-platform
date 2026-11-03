import { defineComponent, type Component, type PropType, type VNodeChild } from 'vue'

/**
 * 单元格内容由使用方的 `cell` 函数产出，必须带上使用方组件的 scopeId，
 * 否则使用方 scoped 样式（.value-cell / .comms-cell 等）匹配不到单元格内节点
 * （`h()` 创建 vnode 时的 scopeId 取自「当前渲染实例」，而 cell 在表格组件实例内执行）。
 * 这里把使用方 vnode 上的 scopeId 注入 CellView，等价 SFC 编译器注入的 `__scopeId`。
 *
 * @param hostScopeId 使用方（页面）组件的 scopeId，由 DataTable.vue 在 setup 里取当前实例取得
 * @returns 可在任意子组件里渲染的单元格渲染器组件
 */
export function createCellView(hostScopeId: string | undefined): Component {
  return Object.assign(
    defineComponent({
      props: { render: { type: Function as PropType<() => VNodeChild>, required: true } },
      setup(p) {
        return () => p.render()
      },
    }),
    { __scopeId: hostScopeId },
  )
}
