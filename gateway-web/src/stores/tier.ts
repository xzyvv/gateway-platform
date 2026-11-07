import { defineStore } from 'pinia'
import { ref, computed, watch } from 'vue'
import type { LicenseState } from '@/api/generated/LicenseState'
import type { LicenseSnapshot } from '@/api/generated/LicenseSnapshot'
import type { QuotaUsage } from '@/api/generated/QuotaUsage'
import type { QuotaAvailable } from '@/api/generated/QuotaAvailable'
import { fetchLicense } from '@/api/license'
import { scopedGet, scopedSet } from '../utils/gatewayStorage'

/** license 档位。v2.0 起仅约束产能配额，不再控制页面可见性。 */
export type Tier = 'core' | 'standard' | 'pro' | 'ultra'

export const TIER_RANK: Record<Tier, number> = {
  core: 0,
  standard: 1,
  pro: 2,
  ultra: 3,
}

export const TIER_LABEL: Record<Tier, string> = {
  core: '核心版',
  standard: '标准版',
  pro: '专业版',
  ultra: '旗舰版',
}

/**
 * 档位说明（v2.1 口径：**采集点位数上限**，不是功能清单）。
 *
 * v1.2 时档位用于功能解锁（抓包 / 波形 / 单步等按档位门控），文案写的是能力范围；
 * v2.0 已删除全部 feature gate（协议扫描 / 抓包 / 单步 / 性能监控等调试能力一律免费，
 * 任何档位都能用），档位**只**约束产能配额。
 *
 * v2.1 起配额**只管点位**：四档唯一差异是采集点位数上限（500 / 1500 / 2500 / 5000）；
 * 并发协议数与北向通道数不限，历史保留天数统一 30 天。故此处只标注点位上限，
 * 避免用户误以为升级档位才能用某些工具或某些协议。
 *
 * 数值与后端 `gateway-core/src/config/license.rs` 的 `DEFAULT_TIER_QUOTAS` 一致；
 * 实际生效配额以 `GET /api/license` 返回的 `quota` 为准（license 自带 quota 优先）。
 */
export const TIER_DESCRIPTION: Record<Tier, string> = {
  core: '500 点',
  standard: '1500 点',
  pro: '2500 点',
  ultra: '5000 点',
}

const STORAGE_KEY = 'gateway_tier'

function loadInitialTier(): Tier {
  try {
    // #6: 按激活网关隔离，避免多网关切换时档位串台
    const raw = scopedGet(STORAGE_KEY)
    if (raw === 'core' || raw === 'standard' || raw === 'pro' || raw === 'ultra') {
      return raw
    }
  } catch { /* ignore */ }
  // v2.0 起档位不再控制页面可见性，开发模式没有理由虚标为最高档
  // （「旗舰版」徽章与真实 license 不符会让现场用户误判授权状态）。
  // 统一从最低档起步，等 /api/license 返回后由 refreshFromLicense 校正。
  return 'core'
}

export const useTierStore = defineStore('tier', () => {
  const current = ref<Tier>(loadInitialTier())

  /**
   * 最近一次 GET /api/license 快照（null = 尚未拉取或拉取失败）。
   * 驱动 Topbar license 状态徽章（生产模式）与 403 拦截提示中的授权编号。
   */
  const licenseState = ref<LicenseState | null>(null)
  const licenseStatus = computed(() => licenseState.value?.status ?? null)

  /**
   * 配额已用量（v2.0）：当前生效配置 / 数据的实际占用。
   * null = 快照未拉到（旧版本网关无此字段）。
   */
  const licenseUsage = ref<QuotaUsage | null>(null)
  /** 配额剩余量（v2.0）：null = 该项无上限；快照未拉到时整体为 null。 */
  const licenseAvailable = ref<QuotaAvailable | null>(null)

  watch(current, (v) => {
    try { scopedSet(STORAGE_KEY, v) } catch { /* ignore */ }
  })

  const rank = computed(() => TIER_RANK[current.value])
  const label = computed(() => TIER_LABEL[current.value])

  /**
   * 拉取并应用 license 快照（§11.1：初始化来源改为 GET /api/license）。
   * - current 一律以 license 为准（含开发模式）：档位已不再控制页面可见性，
   *   开发模式下保留「手动切档」只会让徽章显示一个网关并不具备的档位；
   * - setTier 仍保留给开发期在已加载后临时切换观察 UI。
   *
   * 拉取失败静默返回（网关未就绪 / 匿名禁读 / 旧版本无此端点），不阻断登录与首屏。
   */
  async function refreshFromLicense(): Promise<void> {
    let s: LicenseSnapshot
    try {
      s = await fetchLicense()
    } catch {
      return
    }
    licenseState.value = s
    // v2.0：配额用量随快照一并更新（后端始终返回；旧网关缺字段时按「未知」处理，
    // 前端不伪造数字，避免用户被误导为"已用 0 点"）
    const usage: QuotaUsage | undefined = s.usage
    const available: QuotaAvailable | undefined = s.available
    licenseUsage.value = usage ?? null
    licenseAvailable.value = available ?? null
    current.value = s.tier
  }

  /** 手动切档：仅开发模式可用（生产档位以 license 为准，调用为 no-op） */
  function setTier(t: Tier) {
    if (!import.meta.env.DEV) return
    current.value = t
  }

  return {
    current,
    rank,
    label,
    licenseState,
    licenseStatus,
    licenseUsage,
    licenseAvailable,
    setTier,
    refreshFromLicense,
  }
})
