import { ref, watch, type Ref } from 'vue'
import { useRoute, useRouter, type Router } from 'vue-router'

/**
 * 筛选条件的 URL 持久化。
 *
 * 解决：Hub tab（lazy）切走即销毁，切回后筛选条件与结果全丢。
 * 筛选值读写 route.query（?tab=x&bus=y 不冲突），切回 tab 时从 URL 还原，
 * 且筛选后可刷新页面、可复制链接分享。
 *
 * 值统一为字符串；空串/默认值不写入 query，保持 URL 干净。
 * 内部用 router.replace（不产生历史记录）；同路由 query 变化不触发
 * router.beforeEach 的脏检查（path 不变），与 HubShell 的 ?tab= 同步互不干扰。
 */

/**
 * 同批写入的合并区。
 *
 * 修复（丢参）：原实现每个 key 一个 watch，回调里用「当次读到的 route.query」重建
 * 整个 query 再 router.replace。同一个 flush 内多个 key 同时写入时（如 History
 * 一次还原 point_id/start/end/downsample/granularity 五个条件），各 watch 读到的都是
 * **尚未更新**的 route.query，于是后写覆盖先写——只有最后一个 key 进了 URL，
 * 刷新/分享链接即丢条件。现改为把同批写入累积成 patch，flush 结束后一次性提交。
 */
let pendingPatch: Record<string, string | null> | null = null
let pendingRouter: Router | null = null
let flushScheduled = false

function flushPatch() {
  flushScheduled = false
  const patch = pendingPatch
  const router = pendingRouter
  pendingPatch = null
  if (!patch || !router) return
  const q: Record<string, string> = {}
  for (const [k, v] of Object.entries(router.currentRoute.value.query)) {
    if (typeof v === 'string') q[k] = v
  }
  for (const [k, v] of Object.entries(patch)) {
    // null 表示「回到默认值」：从 URL 中移除该 key
    if (v === null) delete q[k]
    else q[k] = v
  }
  void router.replace({ query: q })
}

/** 安排一次提交；同一批内的后续写入只累积，不重复安排 */
function scheduleFlush() {
  if (flushScheduled) return
  flushScheduled = true
  // 用微任务：Vue 的 watch flush 在同一微任务内跑完所有 watcher，
  // 因此本批所有写入都会先累积进 pendingPatch，再统一提交一次
  queueMicrotask(flushPatch)
}

export function useQueryState(key: string, defaultValue = ''): Ref<string> {
  const route = useRoute()
  const router = useRouter()
  pendingRouter = router
  const state = ref(typeof route.query[key] === 'string' ? (route.query[key] as string) : defaultValue)

  watch(state, (v) => {
    pendingPatch ??= {}
    pendingPatch[key] = v && v !== defaultValue ? v : null
    scheduleFlush()
  })

  // 修复（单向）：原实现只在 setup 读一次 route.query，URL 变化从不同步回 state
  // （浏览器前进/后退、其它入口改 query 都会让界面与 URL 长期不一致）。
  watch(
    () => route.query[key],
    (qv) => {
      const next = typeof qv === 'string' ? qv : defaultValue
      if (next !== state.value) state.value = next
    },
  )

  return state as Ref<string>
}
