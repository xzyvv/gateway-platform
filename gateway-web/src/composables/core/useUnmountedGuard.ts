import { getCurrentInstance, onUnmounted, ref } from 'vue'

/**
 * 组件卸载守卫
 *
 * 统一异步请求中的"组件已卸载则丢弃结果"模式。
 * 每个含异步操作的 Dialog 原本都要写：
 *   let unmounted = false
 *   onUnmounted(() => { unmounted = true })
 *   // ...
 *   if (unmounted) return
 * 使用本 composable 后简化为：
 *   const { isUnmounted } = useUnmountedGuard()
 *   // ...
 *   if (isUnmounted()) return
 *
 * 与 `useRequestGuard` 的分工：
 * - `useRequestGuard` 管的是「旧响应覆盖新响应」（竞态），序号语义；
 * - 本守卫管的是「组件已卸载还往里写状态」（生命周期），布尔语义。
 * 两者互补：前者覆盖请求先后，后者覆盖卸载。
 *
 * @returns isUnmounted 函数，组件卸载后返回 true
 */
export function useUnmountedGuard() {
  const unmounted = ref(false)
  // 在组件 setup 内调用时才注册卸载钩子。被普通函数调用（无宿主实例，
  // 例如 composable 被单测直接调用、或被非组件上下文复用）时跳过，
  // 否则 Vue 会抛 "onUnmounted is called when there is no active component
  // instance" 警告。守卫本身仍然可用——此时它恒为 false。
  if (getCurrentInstance()) {
    onUnmounted(() => {
      unmounted.value = true
    })
  }
  return {
    isUnmounted: () => unmounted.value,
  }
}
