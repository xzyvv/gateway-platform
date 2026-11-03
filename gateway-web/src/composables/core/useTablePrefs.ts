/**
 * 表格显示偏好（密度 + 列显隐）的全站共享入口。
 *
 * 此前这两项偏好只在 Points 单页实现（localStorage key 为页面私有），
 * 带来两个现场问题：
 *   1. 密度是"人体工学"偏好——同一个用户在 1366×768 笔记本上把表调到紧凑，
 *      换到设备管理/数据质量页又变回标准，每进一页都要重设一次。
 *   2. 9 列以上的表在窄屏必然横向滚动，列显隐是刚需，但只有 Points 有这个入口。
 *
 * 设计取舍：
 *   - 密度**全局共享**（一个 ref，所有表格一起变）：用户预期"我调的是这个系统的表"，
 *     而不是"我调的是这一页的表"。按页记忆反而制造新的不一致。
 *   - 列显隐**按表记忆**（key = 表 id）：各页列集合不同，共享一份隐藏列表会
 *     把 A 表的隐藏状态套到 B 表上，产生"这台机器上根本没有的列"。
 *
 * 存储沿用 gatewayStorage 的按网关命名空间隔离（scopedGet/scopedSet）：
 * 桌面端切换网关后，A 网关的表格偏好不应污染 B 网关。
 * 读失败 / 内容损坏 / 隐私模式写入失败一律静默回退默认值，不阻塞页面可用。
 */
import { ref, readonly, type Ref } from 'vue'
import { scopedGet, scopedSet } from '@/utils/gatewayStorage'

export type TableDensity = 'default' | 'compact'

const DENSITY_KEY = 'gateway.table-prefs.density'
const COLS_KEY_PREFIX = 'gateway.table-prefs.cols.'

function loadDensity(): TableDensity {
  // 只有显式存了 'compact' 才算紧凑；其余（含空、损坏）一律标准，
  // 避免一次坏写入让全站表格挤成看不清的样子
  return scopedGet(DENSITY_KEY) === 'compact' ? 'compact' : 'default'
}

/**
 * 模块级单例：所有调用 useTableDensity() 的组件共享同一份状态，
 * 任意一处切换，全站表格（含已挂载的）立即跟随。
 *
 * 这里刻意不用 Pinia：密度是纯 UI 偏好，不参与任何业务判断，也不需要 devtools
 * 调试与持久化插件。用 store 会把一个布尔值塞进持久化体系，反而多一层失效路径。
 * 它与 useProtocol.ts 的模块级共享缓存同理。
 */
// eslint-disable-next-line no-restricted-syntax
const density = ref<TableDensity>(loadDensity())

function setDensity(next: TableDensity): void {
  density.value = next
  try {
    scopedSet(DENSITY_KEY, next)
  } catch { /* 隐私模式写入失败：本次会话内仍生效，只是下次进入丢失 */ }
}

export function useTableDensity() {
  return {
    /** 只读：外部不应直接改，改动请走 setDensity 以保证持久化 */
    density: readonly(density) as Readonly<Ref<TableDensity>>,
    setDensity,
    toggleDensity: () => setDensity(density.value === 'compact' ? 'default' : 'compact'),
  }
}

/** 各表的隐藏列：按表 id 缓存 ref，同一页面多处引用同一份状态 */
const hiddenByTable = new Map<string, { hidden: Ref<string[]>; saved: boolean }>()

/**
 * 窄屏阈值：1366×768 的现场笔记本是主要目标机型。
 * 低于该宽度时，点位列/设备列所有列加起来必然超出可视宽，
 * 与其让用户对着一条看不见的横向滚动条猜「右边还有列」，
 * 不如默认收起低价值列（可在「表格显示设置」里勾回来）。
 */
const NARROW_MAX_WIDTH = 1440

function isNarrowViewport(): boolean {
  return typeof window !== 'undefined' && window.innerWidth < NARROW_MAX_WIDTH
}

function loadHidden(tableId: string): { keys: string[]; saved: boolean } {
  const raw = scopedGet(COLS_KEY_PREFIX + tableId)
  // 从未保存过偏好：交给调用方的窄屏默认值决定（见 useColumnPrefs 的 narrowHidden）
  if (!raw) return { keys: [], saved: false }
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return { keys: [], saved: true }
    // 过滤非字符串项：旧版本或手工改坏的内容不该让 v-for 渲染出 undefined
    return { keys: parsed.filter((k): k is string => typeof k === 'string'), saved: true }
  } catch {
    return { keys: [], saved: true }
  }
}

export interface ColumnPrefsOptions {
  /**
   * 窄屏（< 1440px）下的默认隐藏列。
   * 只在用户**从未改过**该表的列显隐时生效：用户手动调整过的偏好优先，
   * 否则每次换窗口大小都会把用户刻意打开的列又藏起来。
   */
  narrowHidden?: string[]
}

export interface ColumnPrefs {
  /** 隐藏的列 key 数组，直接传给 DataTable 的 hiddenColumns */
  hidden: Ref<string[]>
  isHidden: (key: string) => boolean
  toggle: (key: string) => void
  /** 恢复默认（全部显示） */
  reset: () => void
}

export function useColumnPrefs(tableId: string, options: ColumnPrefsOptions = {}): ColumnPrefs {
  let entry = hiddenByTable.get(tableId)
  if (!entry) {
    const loaded = loadHidden(tableId)
    const initial = !loaded.saved && isNarrowViewport()
      ? [...new Set([...loaded.keys, ...(options.narrowHidden ?? [])])]
      : loaded.keys
    entry = { hidden: ref<string[]>(initial), saved: loaded.saved }
    hiddenByTable.set(tableId, entry)
  }
  const target = entry.hidden

  function persist() {
    entry!.saved = true
    try {
      scopedSet(COLS_KEY_PREFIX + tableId, JSON.stringify(target.value))
    } catch { /* 隐私模式写入失败：不阻塞交互 */ }
  }

  return {
    hidden: target,
    isHidden: (key) => target.value.includes(key),
    toggle(key) {
      target.value = target.value.includes(key)
        ? target.value.filter((k) => k !== key)
        : [...target.value, key]
      persist()
    },
    reset() {
      target.value = []
      persist()
    },
  }
}

/**
 * 测试用：丢弃模块级缓存并从存储重新读取，等价于「重新打开页面」。
 * 不主动清存储——否则无法用它验证持久化是否真的生效。
 */
export function __resetTablePrefsForTests(): void {
  density.value = loadDensity()
  hiddenByTable.clear()
}