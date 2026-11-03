import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

const { listCertificatesMock, uploadCertificateMock, renewCertificateMock, confirmMock } = vi.hoisted(() => ({
  listCertificatesMock: vi.fn(),
  uploadCertificateMock: vi.fn(),
  renewCertificateMock: vi.fn(),
  confirmMock: vi.fn(),
}))

vi.mock('@/api/certificates', () => ({
  listCertificates: listCertificatesMock,
  uploadCertificate: uploadCertificateMock,
  renewCertificate: renewCertificateMock,
}))

vi.mock('@/utils/confirm', () => ({
  confirmAction: confirmMock,
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})

import Certificates from './Certificates.vue'
import { ElMessage } from 'element-plus'

/** Certificates 测试：证书重新生成（POST）失败必须给出动作语境提示 */

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
  document.body.innerHTML = ''
})

describe('Certificates', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listCertificatesMock.mockResolvedValue([])
    confirmMock.mockResolvedValue(true)
  })

  it('重新生成失败时 ElMessage.error 含「重新生成证书失败」，且不提示成功', async () => {
    renewCertificateMock.mockRejectedValueOnce(new Error('500'))
    const wrapper = mount(Certificates, { attachTo: document.body })
    wrappers.push(wrapper)
    await flushPromises()

    const renewBtn = wrapper.findAll('button').find((b) => b.text().includes('重新生成自签名'))
    expect(renewBtn).toBeTruthy()
    await renewBtn!.trigger('click')
    await flushPromises()

    expect(renewCertificateMock).toHaveBeenCalledTimes(1)
    // POST 重新生成全局静默，组件必须自己提示失败
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('重新生成证书失败'))
    // 失败不得冒充成功
    expect(ElMessage.success).not.toHaveBeenCalled()
  })
})
