import { ElMessageBox } from 'element-plus'

/**
 * 全局未保存更改（脏状态）注册表
 *
 * 写操作页面/tab 在数据变更时 mark()，保存成功后 reset()，卸载时 unregister()。
 * 路由离开（router.beforeEach，path 变化时）与 Hub tab 切换（HubShell before-leave）
 * 统一检查本表，有脏则弹确认，防止未保存配置静默丢失。
 *
 * 注意：同一路由内的 query 变化（Hub tab 切换）由 HubShell 拦截，
 * beforeEach 只拦 path 变化，避免同一动作双重弹窗。
 *
 * Bug-W6 (fix)：多 tab 同时编辑场景下，`dirtyKeys.clear()` 会清空**所有**路由的脏标记。
 * 改用按 scope（路由 path）分桶：只有当前路由的脏标记会被清，其他 tab 的修改保留。
 *
 * 设计要点：
 * - 注册时绑定 `scope`（通常 = 路由 path）；卸载时 unregister 即删除 key。
 * - `confirmDiscardDirty(currentScope)`：弹框前先按 `currentScope` 过滤，
 *   用户点确认时**只清当前 scope 的 key**，不污染其他 tab 的未保存数据。
 * - 兼容旧 API（无 scope）：scope 默认为空串 `''`，所有无 scope 的注册归同一桶，
 *   行为退化为旧实现（避免破坏既有调用点，迁移期间两个 API 并存）。
 */
let seq = 0

/**
 * scope -> 该 scope 下的脏 key 集合。
 *
 * 注意：保留空串 bucket 兼容旧 API（未传 scope 的旧代码路径）；
 * 未来所有调用方迁移到带 scope 后，可以单独清空 ''。
 */
const dirtyKeysByScope = new Map<string, Set<number>>()

export interface DirtyGuard {
  /** 标记存在未保存更改（表单 input/change 或 watch 中调用） */
  mark(): void
  /** 保存成功后清除脏标记 */
  reset(): void
  /** 组件卸载时注销（onUnmounted 调用，防泄漏） */
  unregister(): void
}

function getOrCreateBucket(scope: string): Set<number> {
  let bucket = dirtyKeysByScope.get(scope)
  if (!bucket) {
    bucket = new Set<number>()
    dirtyKeysByScope.set(scope, bucket)
  }
  return bucket
}

/**
 * 浏览器刷新/关标签页保护。
 *
 * 路由切换与 Hub tab 切换由 beforeEach / HubShell 拦截，但 F5、Ctrl+R、关闭标签页
 * 不走路由钩子；没有 beforeunload 时，弹窗里改到一半的数据会直接丢失。
 *
 * 按需挂载：只有存在脏标记时才注册监听，全部保存/注销后立即摘下，避免常驻监听。
 */
let beforeUnloadHandler: ((e: BeforeUnloadEvent) => void) | null = null

function syncBeforeUnload(): void {
  if (typeof window === 'undefined') return
  const dirty = isAnyDirty()
  if (dirty && !beforeUnloadHandler) {
    beforeUnloadHandler = (e: BeforeUnloadEvent) => {
      // 现代浏览器会忽略自定义文案，但 preventDefault + returnValue 仍是触发提示的必要条件
      e.preventDefault()
      e.returnValue = ''
    }
    window.addEventListener('beforeunload', beforeUnloadHandler)
  } else if (!dirty && beforeUnloadHandler) {
    window.removeEventListener('beforeunload', beforeUnloadHandler)
    beforeUnloadHandler = null
  }
}

/**
 * 注册一个脏标记守卫。
 *
 * @param scope 路由 path 或其他隔离键（默认 '' 表示不分组/兼容旧 API）
 *   同一 scope 内的 mark/reset 都只影响该 scope；弹框 `confirmDiscardDirty(scope)`
 *   也只清空指定 scope 的脏标记。
 */
export function registerDirtyGuard(scope: string = ''): DirtyGuard {
  const key = ++seq
  const bucket = getOrCreateBucket(scope)
  return {
    mark: () => {
      bucket.add(key)
      syncBeforeUnload()
    },
    reset: () => {
      bucket.delete(key)
      syncBeforeUnload()
    },
    unregister: () => {
      bucket.delete(key)
      syncBeforeUnload()
    },
  }
}

/**
 * 当前是否存在未保存更改（任一 scope）。
 *
 * 兼容旧 API：仍允许 `isAnyDirty()` 判断全局；多 scope 场景下一般不需要此判断
 * （beforeEach/HubShell 已经把 currentScope 传给 `confirmDiscardDirty`）。
 */
export function isAnyDirty(): boolean {
  for (const bucket of dirtyKeysByScope.values()) {
    if (bucket.size > 0) {
      return true
    }
  }
  return false
}

/**
 * 指定 scope 内是否存在脏标记
 */
export function isScopeDirty(scope: string): boolean {
  return (dirtyKeysByScope.get(scope)?.size ?? 0) > 0
}

/**
 * 有未保存更改时弹确认框。
 *
 * Bug-W6 (fix)：只清空当前 `scope` 的脏标记，保留其他 tab / scope 的未保存数据。
 *
 * @param currentScope 当前路由 path 或上下文 scope；只对当前 scope 的脏标记操作
 * @returns true = 用户确认放弃当前 scope 的更改（已清空）可以离开；false = 留在当前页
 */
export async function confirmDiscardDirty(currentScope: string): Promise<boolean> {
  if (!isScopeDirty(currentScope)) {
    return true
  }
  try {
    await ElMessageBox.confirm('当前有未保存的更改，离开后将丢失。', '未保存的更改', {
      type: 'warning',
      confirmButtonText: '放弃更改并离开',
      cancelButtonText: '留在本页',
      autofocus: false,
    })
    // Bug-W6: 只清当前 scope 的脏标记，其他 tab 数据完整保留
    const bucket = dirtyKeysByScope.get(currentScope)
    if (bucket) {
      bucket.clear()
      syncBeforeUnload()
      // bucket 空时是否删除 key？这层选择：
      // - 删除：避免长期累积 scope 名（路由切换会自然重连）
      // - 保留：避免 isScopeDirty 频繁重建 Set 的小开销
      // 选"保留"——脏数据本来就是边界态，长期累积 scope 数有限（最多路由数）
    }
    return true
  } catch {
    return false
  }
}

/** 测试用：清空所有脏标记 */
export function __resetAllDirtyState(): void {
  dirtyKeysByScope.clear()
  syncBeforeUnload()
}
