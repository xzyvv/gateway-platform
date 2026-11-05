import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount } from '@vue/test-utils'
import { nextTick } from 'vue'
import LicenseDialog from './LicenseDialog.vue'
import { useTierStore } from '../../stores/tier'

/** LicenseDialog 测试：挂载冒烟 + 设备码复制 + .lic 导入成功/失败中文反馈 */

const { fetchLicenseMock, importLicenseMock } = vi.hoisted(() => ({
  fetchLicenseMock: vi.fn(),
  importLicenseMock: vi.fn(),
}))

// vi.mock 工厂替换 api/license 模块（fetchLicense / importLicense 均走 mock）
vi.mock('@/api/license', () => ({
  fetchLicense: fetchLicenseMock,
  importLicense: importLicenseMock,
}))

const SNAPSHOT = {
  present: true,
  tier: 'pro',
  quota: {
    max_points: 1500,
    max_protocols: 8,
    storage_days: 90,
    northbound_channels: 2,
  },
  quota_source: 'license',
  status: 'ok',
  expires_at: 1_790_000_000,
  license_id: 'LIC-2026-0001',
  customer: '某某自动化',
  machine: {
    machine_key: 'a'.repeat(64),
    onboard_mac: 'b'.repeat(64),
    storage_serial: null,
    device_serial: null,
  },
}

const writeText = vi.fn().mockResolvedValue(undefined)

const allWrappers: ReturnType<typeof mount>[] = []
afterEach(() => {
  for (const w of allWrappers) w.unmount()
  allWrappers.length = 0
  document.body.innerHTML = ''
  vi.clearAllMocks()
})

/** 挂载后经 setProps 触发 open（el-dialog 懒渲染，@open 驱动快照拉取） */
async function mountDialog() {
  const wrapper = mount(LicenseDialog, {
    props: { modelValue: false },
    attachTo: document.body,
  })
  allWrappers.push(wrapper)
  await nextTick()
  // 注：build 脚本走纯 tsc（.vue shim 下组件 props 推断为 {}），用 never 断言绕过；
  // vue-tsc / Volar 下本就有精确类型，断言不影响运行时
  await wrapper.setProps({ modelValue: true } as never)
  // @open → onOpen → fetchLicense promise → 渲染
  await new Promise((r) => setTimeout(r, 0))
  await nextTick()
  return wrapper
}

describe('LicenseDialog', () => {
  beforeEach(() => {
    Object.defineProperty(navigator, 'clipboard', {
      value: { writeText },
      configurable: true,
    })
    fetchLicenseMock.mockResolvedValue(SNAPSHOT)
  })

  it('打开后展示当前授权状态与设备码（缺失项不渲染）', async () => {
    await mountDialog()
    expect(document.body.textContent).toContain('LIC-2026-0001')
    expect(document.body.textContent).toContain('某某自动化')
    // v2.0 产能配额展示
    expect(document.querySelector('.lic-quota')?.textContent).toContain('采集点数')
    expect(document.querySelector('.lic-quota')?.textContent).toContain('1500')
    expect(document.querySelector('.lic-quota')?.textContent).toContain('上行通道数')
    expect(document.querySelector('.lic-quota')?.textContent).toContain('2')
    expect(document.body.textContent).toContain('主设备码')
    expect(document.querySelector('.lic-machine-value')?.textContent).toContain('a'.repeat(64))
    const rows = document.querySelectorAll('.lic-machine-row')
    expect(rows).toHaveLength(2) // 主键 + 板载网卡；storage/device 缺失不渲染
    expect(document.body.textContent).not.toContain('备用·存储序列号')
  })

  it('复制全部设备码：写入主键 + 备用行（缺失项不拼入）', async () => {
    await mountDialog()
    const btn = Array.from(document.querySelectorAll('button')).find(
      (b) => b.textContent?.includes('复制全部设备码'),
    ) as HTMLButtonElement
    expect(btn).toBeTruthy()
    btn.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }))
    await new Promise((r) => setTimeout(r, 0))
    expect(writeText).toHaveBeenCalledTimes(1)
    const text = writeText.mock.calls[0]![0] as string
    expect(text).toContain(`主设备码: ${'a'.repeat(64)}`)
    expect(text).toContain(`备用设备码: ${'b'.repeat(64)}`)
  })

  it('导入成功：展示成功提示并刷新全局授权快照', async () => {
    const imported = { ...SNAPSHOT, tier: 'ultra', license_id: 'LIC-2026-0002' }
    // 调用序：open 拉取 → SNAPSHOT；导入成功后 refreshFromLicense 再次 GET → imported
    fetchLicenseMock.mockResolvedValueOnce(SNAPSHOT).mockResolvedValue(imported)
    importLicenseMock.mockResolvedValue(imported)
    await mountDialog()
    const input = document.querySelector('.lic-file-input') as HTMLInputElement
    Object.defineProperty(input, 'files', {
      value: [new File(['LIC-CONTENT'], 'gw.lic')],
      configurable: true,
    })
    input.dispatchEvent(new Event('change'))
    await new Promise((r) => setTimeout(r, 0))
    await nextTick()
    expect(importLicenseMock).toHaveBeenCalledWith('LIC-CONTENT')
    expect(document.querySelector('.lic-alert')?.textContent).toContain('授权导入成功')
    // 等待 fire-and-forget 的 refreshFromLicense 链路完成
    await new Promise((r) => setTimeout(r, 0))
    const tierStore = useTierStore()
    expect(tierStore.licenseState?.license_id).toBe('LIC-2026-0002')
  })

  it('导入失败：行内透传后端中文原因（不落 toast 依赖）', async () => {
    importLicenseMock.mockRejectedValue({
      response: { data: { message: '授权文件与本设备不匹配：请核对本机设备码后联系供应商重新签发' } },
    })
    await mountDialog()
    const input = document.querySelector('.lic-file-input') as HTMLInputElement
    Object.defineProperty(input, 'files', {
      value: [new File(['BAD'], 'bad.lic')],
      configurable: true,
    })
    input.dispatchEvent(new Event('change'))
    await new Promise((r) => setTimeout(r, 0))
    await nextTick()
    const alert = document.querySelector('.lic-alert')
    expect(alert?.textContent).toContain('授权文件与本设备不匹配')
  })

  it('获取快照失败：展示获取失败提示', async () => {
    fetchLicenseMock.mockRejectedValue(new Error('gateway not ready'))
    await mountDialog()
    expect(document.body.textContent).toContain('授权状态获取失败')
  })

  it('配额降级（tier_quotas 漏配该档）：显式提示而非静默显示低配额', async () => {
    fetchLicenseMock.mockResolvedValue({
      ...SNAPSHOT,
      quota_source: 'tier_missing',
      quota: {
        max_points: 100,
        max_protocols: 1,
        storage_days: 7,
        northbound_channels: 1,
      },
    })
    await mountDialog()
    const alert = document.querySelector('.lic-quota-alert')
    expect(alert?.textContent).toContain('专业版')
    expect(alert?.textContent).toContain('tier_quotas')
  })

  it('正常来源（license 自带 quota）：不展示降级告警', async () => {
    await mountDialog()
    expect(document.querySelector('.lic-quota-alert')).toBeNull()
  })
})
