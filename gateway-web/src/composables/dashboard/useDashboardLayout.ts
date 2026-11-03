/**
 * 自定义仪表盘布局
 * 允许用户控制各模块的显示顺序与可见性，持久化到 localStorage
 */
import { ref, watch } from 'vue'
import { globalGet, globalSet } from '@/utils/gatewayStorage'

/** 仪表盘布局是「人」的偏好，切网关不重置，故走全局入口；由本模块独占访问 */
const STORAGE_KEY = 'gw_dashboard_layout'

export interface DashboardSection {
  key: string
  label: string
  visible: boolean
  order: number
}

const DEFAULT_SECTIONS: DashboardSection[] = [
  { key: 'metrics', label: '指标卡片', visible: true, order: 0 },
  // 异常聚合放在最前：现场先看有没有问题，再看细节指标
  { key: 'attention', label: '需要关注', visible: true, order: 1 },
  { key: 'trend', label: '健康趋势', visible: true, order: 2 },
  { key: 'gauges', label: '仪表盘', visible: true, order: 3 },
  { key: 'health', label: '组件健康', visible: true, order: 4 },
  { key: 'devices', label: '设备列表', visible: true, order: 5 },
]

function load(): DashboardSection[] {
  const fallback = JSON.parse(JSON.stringify(DEFAULT_SECTIONS))
  try {
    const raw = globalGet(STORAGE_KEY)
    if (!raw) return fallback
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) return fallback
    // 字段完整性校验：脏数据（缺 key）一律 fallback
    const saved = parsed
      .filter((s: unknown): s is Record<string, unknown> => !!s && typeof s === 'object' && typeof (s as Record<string, unknown>).key === 'string')
      .map((s: Record<string, unknown>): DashboardSection => {
        const k = s.key as string
        return {
          key: k,
          label: typeof s.label === 'string' ? s.label : k,
          visible: typeof s.visible === 'boolean' ? s.visible : true,
          order: typeof s.order === 'number' ? s.order : 0,
        }
      })
    // 合并默认项，确保新模块能出现
    const map = new Map(saved.map((s) => [s.key, s]))
    return DEFAULT_SECTIONS.map((d) => map.get(d.key) || d)
      .sort((a, b) => a.order - b.order)
  } catch {
    return fallback
  }
}

// @singleton: 仪表盘布局是用户偏好，需跨组件共享（Dashboard + 嵌入组件），有意使用模块级 ref
// 持久化到 localStorage，后续如需 devtools 追踪可迁移到 Pinia store
// eslint-disable-next-line no-restricted-syntax
const sections = ref<DashboardSection[]>(load())

// 防抖写入：用户连续拖拽排序时避免频繁 localStorage I/O
let saveTimer: ReturnType<typeof setTimeout> | null = null
watch(sections, (v) => {
  if (saveTimer) clearTimeout(saveTimer)
  saveTimer = setTimeout(() => {
    saveTimer = null
    try { globalSet(STORAGE_KEY, JSON.stringify(v)) } catch { /* ignore */ }
  }, 200)
}, { deep: true })

export function useDashboardLayout() {
  function toggle(key: string) {
    const s = sections.value.find((x) => x.key === key)
    if (s) s.visible = !s.visible
  }

  function move(key: string, dir: -1 | 1) {
    const idx = sections.value.findIndex((x) => x.key === key)
    const target = idx + dir
    if (idx < 0 || target < 0 || target >= sections.value.length) return
    const a = sections.value[idx]!
    const b = sections.value[target]!
    ;[a.order, b.order] = [b.order, a.order]
    sections.value.sort((x, y) => x.order - y.order)
  }

  function reset() {
    sections.value = JSON.parse(JSON.stringify(DEFAULT_SECTIONS))
  }

  return {
    // state
    sections,
    // methods
    toggle,
    move,
    reset,
  }
}


