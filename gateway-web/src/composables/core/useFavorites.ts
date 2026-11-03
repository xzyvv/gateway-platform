import { ref, watch } from 'vue'
import { scopedGet, scopedSet } from '../../utils/gatewayStorage'

/**
 * 点位收藏 / 关注列表 — 基于 localStorage 持久化
 * 适用于 Realtime 视图置顶显示常用点位
 */
const STORAGE_KEY = 'gw_favorite_points'
const MAX_FAVORITES = 200

function readFavorites(): Set<string> {
  try {
    // #6: 按激活网关隔离，点位 id 在不同网关间可能有歧义
    const raw = scopedGet(STORAGE_KEY)
    if (!raw) return new Set()
    const parsed = JSON.parse(raw)
    // 过滤脏数据：只保留 string 元素，避免下游 .has(id) 出错
    return new Set(Array.isArray(parsed) ? parsed.filter((v: unknown) => typeof v === 'string') : [])
  } catch {
    return new Set()
  }
}

// @singleton: 收藏列表需跨页面共享（Realtime 置顶 + 列表页标记），有意使用模块级 ref
// 后续如需 devtools 追踪可迁移到 Pinia store
// eslint-disable-next-line no-restricted-syntax
const favorites = ref<Set<string>>(readFavorites())

function persist() {
  try {
    scopedSet(STORAGE_KEY, JSON.stringify([...favorites.value]))
  } catch {
    // ignore
  }
}

watch(favorites, persist, { deep: false })

export function useFavorites() {
  function isFavorite(id: string): boolean {
    return favorites.value.has(id)
  }

  function toggleFavorite(id: string): boolean {
    if (favorites.value.has(id)) {
      favorites.value.delete(id)
    } else {
      // 超过上限不再加入，避免 localStorage 撑爆
      if (favorites.value.size >= MAX_FAVORITES) return false
      favorites.value.add(id)
    }
    // 触发响应式更新
    favorites.value = new Set(favorites.value)
    return favorites.value.has(id)
  }

  function clearFavorites() {
    favorites.value = new Set()
  }

  return {
    // state
    favorites,
    // methods
    isFavorite,
    toggleFavorite,
    clearFavorites,
  }
}


