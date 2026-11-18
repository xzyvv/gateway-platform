import { getCurrentInstance, onUnmounted } from 'vue'

/**
 * 请求序号守卫：只接受「最新一次」请求的结果。
 *
 * 历史问题：多处手写同一个模式——「拿到响应后，若期间又发了新请求，
 * 就丢弃这次旧响应」。手写散落在 6 处，每处写法略有不同
 * （有的判 `seq !== counter`，有的判 `seq === counter` 才写，
 * 有的还混着 `unmounted` 判断），容易漏掉某个分支，导致旧响应覆盖新数据。
 *
 * 典型场景：用户快速切换设备 / 切换筛选条件 / 连续点两次刷新，
 * 先发的请求后到 → 表格显示上一台设备的数据，而界面上的选择框已是新的。
 *
 * 用法：
 *   const { next, isCurrent } = useRequestGuard()
 *   async function load() {
 *     const seq = next()
 *     const data = await fetchSomething()
 *     if (!isCurrent(seq)) return   // 已有更新的请求发出 → 丢弃本次结果
 *     target.value = data
 *   }
 *
 * `isCurrent` 同时覆盖「组件已卸载」——卸载后所有在途响应都会被丢弃，
 * 不再往已销毁的组件里写状态。
 */
export function useRequestGuard() {
  let seq = 0
  let unmounted = false

  /**
   * 开启新一轮请求，返回本轮的序号。
   * 调用即作废此前所有在途请求的结果。
   */
  function next(): number {
    return ++seq
  }

  /**
   * 判断某个序号是否仍是「当前这一轮」。
   * 组件卸载后恒为 false —— 在途响应一律丢弃。
   */
  function isCurrent(requestSeq: number): boolean {
    return !unmounted && requestSeq === seq
  }

  // 在组件 setup 内调用时，卸载即作废所有在途请求。
  // 被普通函数调用（无宿主实例）时跳过注册，避免 Vue 的
  // "onUnmounted is called when there is no active component instance" 警告。
  if (getCurrentInstance()) {
    onUnmounted(() => {
      unmounted = true
      // 作废所有在途请求
      seq++
    })
  }

  return {
    next,
    isCurrent,
  }
}
