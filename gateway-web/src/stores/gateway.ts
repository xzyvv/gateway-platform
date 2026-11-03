import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getConfig } from '@/api/config'
import { getStatus, getVersion, healthReady } from '@/api/status'
import type { GatewayConfig, RunStatus, VersionInfo, HealthReady } from '@/api/types'
import { useNotificationsStore } from './notifications'
import { extractErrorMessage } from '../utils/errorHandler'

/**
 * 网关共享状态 Store
 *
 * 集中管理跨页面共享的网关级数据：配置、运行状态、版本信息、健康状态。
 * 深化点：
 * - 每个资源独立的 loading / error 状态
 * - Stale-While-Revalidate：先返回缓存，后台静默刷新
 * - 显式缓存失效（配置变更后调用 invalidate）
 * - 错误统一上报 notifications store
 */
export const useGatewayStore = defineStore('gateway', () => {
  const config = ref<GatewayConfig | null>(null)
  const status = ref<RunStatus | null>(null)
  const version = ref<VersionInfo | null>(null)
  const health = ref<HealthReady | null>(null)

  const loading = ref(false)
  const errors = ref<Record<string, string>>({})
  const lastUpdated = ref<Record<string, number>>({})

  const CACHE_TTL_MS = 5000

  const notify = useNotificationsStore()

  function setError(key: string, msg: string) {
    errors.value[key] = msg
  }
  function clearError(key: string) {
    delete errors.value[key]
  }

  /** 加载网关配置（SWR：有缓存先返回，后台刷新） */
  async function loadConfig(force = false): Promise<GatewayConfig | null> {
    const key = 'config'
    const cached = config.value
    const fresh = cached && !force && Date.now() - (lastUpdated.value[key] ?? 0) < CACHE_TTL_MS
    if (fresh) return cached

    // SWR: 有缓存但已过期，先返回旧值，后台刷新
    const stale = cached
    clearError(key)
    ;(async () => {
      try {
        config.value = await getConfig()
        lastUpdated.value[key] = Date.now()
      } catch (e) {
        const msg = extractErrorMessage(e)
        setError(key, msg)
        notify.error(`加载配置失败: ${msg}`)
      }
    })()
    return stale ?? config.value
  }

  async function loadStatus(): Promise<RunStatus | null> {
    const key = 'status'
    clearError(key)
    try {
      status.value = await getStatus()
      lastUpdated.value[key] = Date.now()
    } catch (e) {
      const msg = extractErrorMessage(e)
      setError(key, msg)
    }
    return status.value
  }

  async function loadVersion(): Promise<VersionInfo | null> {
    const key = 'version'
    if (version.value) return version.value
    clearError(key)
    try {
      version.value = await getVersion()
      lastUpdated.value[key] = Date.now()
    } catch (e) {
      const msg = extractErrorMessage(e)
      setError(key, msg)
    }
    return version.value
  }

  async function loadHealth(): Promise<HealthReady | null> {
    const key = 'health'
    clearError(key)
    try {
      health.value = await healthReady()
      lastUpdated.value[key] = Date.now()
    } catch (e) {
      const msg = extractErrorMessage(e)
      setError(key, msg)
    }
    return health.value
  }

  /** 一键刷新所有网关级数据 */
  async function refreshAll() {
    loading.value = true
    try {
      await Promise.all([
        loadConfig(true),
        loadStatus(),
        loadVersion(),
        loadHealth(),
      ])
    } finally {
      loading.value = false
    }
  }

  /** 使缓存失效（配置变更后调用） */
  function invalidate(keys?: string[]) {
    const targets = keys ?? ['config', 'status', 'version', 'health']
    for (const k of targets) {
      lastUpdated.value[k] = 0
    }
  }

  return {
    config,
    status,
    version,
    health,
    loading,
    errors,
    lastUpdated,
    loadConfig,
    loadStatus,
    loadVersion,
    loadHealth,
    refreshAll,
    invalidate,
  }
})
