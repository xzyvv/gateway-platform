import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

const { getOtaStatusMock, otaUploadMock, otaRollbackMock, confirmMock } = vi.hoisted(() => ({
  getOtaStatusMock: vi.fn(),
  otaUploadMock: vi.fn(),
  otaRollbackMock: vi.fn(),
  confirmMock: vi.fn(),
}))

vi.mock('@/api/ota', () => ({
  getOtaStatus: getOtaStatusMock,
  otaUpload: otaUploadMock,
  otaRollback: otaRollbackMock,
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

import Ota from './Ota.vue'
import { ElMessage } from 'element-plus'

/** Ota 测试：固件回滚（POST）失败必须给出动作语境提示 */

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
  document.body.innerHTML = ''
})

describe('Ota', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    getOtaStatusMock.mockResolvedValue({ progress: 0, status: 'idle' })
    confirmMock.mockResolvedValue(true)
  })

  it('回滚失败时 ElMessage.error 含「固件回滚失败」，且不提示成功', async () => {
    otaRollbackMock.mockRejectedValueOnce(new Error('500'))
    const wrapper = mount(Ota, { attachTo: document.body })
    wrappers.push(wrapper)
    await flushPromises()

    const rollbackBtn = wrapper.findAll('button').find((b) => b.text().includes('回滚固件'))
    expect(rollbackBtn).toBeTruthy()
    await rollbackBtn!.trigger('click')
    await flushPromises()

    expect(otaRollbackMock).toHaveBeenCalledTimes(1)
    // POST 回滚全局静默，组件必须自己提示失败
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('固件回滚失败'))
    // 失败不得冒充成功
    expect(ElMessage.success).not.toHaveBeenCalled()
  })
})
