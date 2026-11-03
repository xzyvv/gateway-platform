/**
 * tier store 单测（license 迁移后，§11.1）
 *
 * 覆盖：
 * - refreshFromLicense 更新 licenseState 快照；拉取失败静默
 * - DEV / 生产两种模式下 current 一律以 license 为准（档位不再虚标最高档）
 * - setTier 仅开发模式生效
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useTierStore, TIER_LABEL } from './tier'
import type { LicenseState } from '@/api/generated/LicenseState'

const { fetchLicenseMock } = vi.hoisted(() => ({
  fetchLicenseMock: vi.fn(),
}))
// mock 工厂会被提升到文件顶部，引用顶层变量必须走 vi.hoisted（项目既有教训）
vi.mock('@/api/license', () => ({
  fetchLicense: fetchLicenseMock,
}))

function licState(over: Partial<LicenseState> = {}): LicenseState {
  return {
    present: true,
    tier: 'pro',
    quota: {
      max_points: null,
      max_protocols: null,
      storage_days: null,
      northbound_channels: null,
    },
    status: 'ok',
    expires_at: null,
    license_id: 'LIC-TEST-0001',
    customer: null,
    ...over,
  }
}

describe('tier store（license 驱动）', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    localStorage.removeItem('gateway_tier')
    fetchLicenseMock.mockReset()
  })

  afterEach(() => {
    // 恢复 vitest 默认 DEV=true，避免影响其他用例
    ;(import.meta.env as { DEV: boolean }).DEV = true
  })

  it('refreshFromLicense 更新快照；DEV 下同样以 license 档位为准', async () => {
    const store = useTierStore()
    fetchLicenseMock.mockResolvedValue(licState({ tier: 'core' }))
    expect(store.current).toBe('core')

    await store.refreshFromLicense()

    expect(store.licenseState?.license_id).toBe('LIC-TEST-0001')
    expect(store.licenseStatus).toBe('ok')
    // 档位不再控制可见性：徽章必须如实反映 license，而不是恒显最高档
    expect(store.current).toBe('core')
  })

  it('DEV 下 setTier 仍可临时切换（模拟用），但再次拉取 license 会被校正', async () => {
    const store = useTierStore()
    store.setTier('ultra')
    expect(store.current).toBe('ultra')

    fetchLicenseMock.mockResolvedValue(licState({ tier: 'standard' }))
    await store.refreshFromLicense()
    expect(store.current).toBe('standard')
  })

  it('生产模式：current 严格由 license 驱动（含降级 core）', async () => {
    ;(import.meta.env as { DEV: boolean }).DEV = false
    const store = useTierStore()
    fetchLicenseMock.mockResolvedValue(licState({ present: false, tier: 'core', status: 'expired' }))

    await store.refreshFromLicense()

    expect(store.current).toBe('core')
    expect(store.label).toBe(TIER_LABEL.core)
  })

  it('生产模式：setTier 为 no-op，档位以 license 为准', async () => {
    ;(import.meta.env as { DEV: boolean }).DEV = false
    const store = useTierStore()
    fetchLicenseMock.mockResolvedValue(licState({ tier: 'pro' }))
    await store.refreshFromLicense()
    expect(store.current).toBe('pro')

    store.setTier('ultra')
    expect(store.current).toBe('pro')
  })

  it('拉取失败静默：快照保持 null，current 不变', async () => {
    const store = useTierStore()
    const before = store.current
    fetchLicenseMock.mockRejectedValue(new Error('gateway not ready'))

    await expect(store.refreshFromLicense()).resolves.toBeUndefined()

    expect(store.licenseState).toBeNull()
    expect(store.current).toBe(before)
  })
})
