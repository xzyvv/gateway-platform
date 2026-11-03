import { onUnmounted, ref } from 'vue'

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
 * @returns isUnmounted 函数，组件卸载后返回 true
 */
export function useUnmountedGuard() {
  const unmounted = ref(false)
  onUnmounted(() => {
    unmounted.value = true
  })
  return {
    isUnmounted: () => unmounted.value,
  }
}
