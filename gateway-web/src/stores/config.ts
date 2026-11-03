import { defineStore } from 'pinia'
import { ref } from 'vue'
import { getConfig, postConfig, reloadConfig } from '@/api/config'
import type { GatewayConfig } from '@/api/types'
import { useNotificationsStore } from './notifications'
import { extractErrorMessage } from '../utils/errorHandler'

/**
 * 配置 Store
 *
 * 管理网关配置的读取/写入/热重载。
 * - 乐观更新：提交后先更新本地配置，失败回滚
 * - 写操作后使 gateway store 缓存失效
 * - 错误统一上报 notifications
 */
export const useConfigStore = defineStore('config', () => {
  const config = ref<GatewayConfig | null>(null)
  const loading = ref(false)
  const saving = ref(false)
  const error = ref<string | null>(null)
  const lastUpdated = ref(0)
  const CACHE_TTL_MS = 5000

  const notify = useNotificationsStore()

  async function load(force = false): Promise<GatewayConfig | null> {
    if (config.value && !force && Date.now() - lastUpdated.value < CACHE_TTL_MS) {
      return config.value
    }
    loading.value = true
    error.value = null
    try {
      config.value = await getConfig()
      lastUpdated.value = Date.now()
    } catch (e) {
      const msg = extractErrorMessage(e)
      error.value = msg
    } finally {
      loading.value = false
    }
    return config.value
  }

  async function save(newConfig: GatewayConfig): Promise<boolean> {
    const old = config.value
    // 乐观更新
    config.value = newConfig
    saving.value = true
    try {
      await postConfig(newConfig)
      lastUpdated.value = Date.now()
      notify.success('配置已保存并热重载')
      return true
    } catch (e) {
      // 回滚
      config.value = old
      const msg = extractErrorMessage(e)
      notify.error(`保存配置失败: ${msg}`)
      return false
    } finally {
      saving.value = false
    }
  }

  async function reload(): Promise<boolean> {
    try {
      await reloadConfig()
      lastUpdated.value = 0
      await load(true)
      notify.success('配置已重新加载')
      return true
    } catch (e) {
      const msg = extractErrorMessage(e)
      notify.error(`热重载失败: ${msg}`)
      return false
    }
  }

  function invalidate() {
    lastUpdated.value = 0
  }

  return {
    config,
    loading,
    saving,
    error,
    load,
    save,
    reload,
    invalidate,
  }
})
