/**
 * 列表行的稳定 key 分配器（按对象引用）。
 *
 * # 为什么需要
 *
 * Vue 的 `v-for` key 若用数组下标，当列表行**可删除**时会出问题：
 * 删掉中间一行后，下标整体前移，Vue 复用错误的 DOM，表单里已填的值
 * 会错位到隔壁行——用户看到「删了第 2 行，第 3 行的内容跑到了第 2 行」。
 *
 * 正确做法是用一个稳定的、与行内容无关的标识。但很多列表装的是**后端返回的
 * 数据对象**（`OpcuaConfig.users`、`BusConfig.buses`），不能为了 key 往
 * 这些对象里塞字段——那会被原样提交给后端，污染契约。
 *
 * 本工具用 WeakMap 按对象引用分配自增 id：
 * - 同一对象永远拿到同一个 key → 删除中间行不影响其它行的 key；
 * - 不用改数据对象本身 → 后端契约不受影响；
 * - WeakMap 弱引用 → 数据对象被回收时 key 自动消失，不会泄漏。
 *
 * 用法：
 *     const keyOf = createRowKey()
 *     // 模板：<div v-for="u in form.users" :key="keyOf(u)">
 *
 * 注意：key 只在**同一份列表数据**内唯一。跨列表复用同一 keyOf 也没问题
 * （不同对象拿到的 id 不同），但把一个列表的对象挪到另一个列表会改变 key，
 * 这在「增删行」场景下是期望行为（视为新行）。
 */

/** 行 key 分配器：传入行对象，返回与该对象绑定的稳定数字 key */
export type RowKeyOf<T> = (row: T) => number

/**
 * 创建一个行 key 分配器。
 *
 * @param initial 起始 id（默认 1）；显式指定便于测试里断言具体值
 */
export function createRowKey(initial = 1): RowKeyOf<object> {
  const ids = new WeakMap<object, number>()
  let seq = initial
  return (row: object) => {
    let id = ids.get(row)
    if (id === undefined) {
      id = seq++
      ids.set(row, id)
    }
    return id
  }
}
