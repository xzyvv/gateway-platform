/**
 * copyText 的契约：**成功提示必须对应真实的复制成功**。
 *
 * 背景：现场网关多以 `http://<ip>:<port>` 直连，属于非安全上下文，多数浏览器
 * 此时不提供 `navigator.clipboard`。原先各页面各自直写 `navigator.clipboard?.writeText()`
 * 再无条件 `ElMessage.success('已复制')`——可选链把「接口不存在」静默吞掉，
 * 用户看到「已复制」而剪贴板是空的，粘到工单里才发现没内容。
 *
 * 这里锁住四件事：
 *   1. 有 clipboard API 时正常走它
 *   2. 无 clipboard API（http 非安全上下文）时回退 execCommand 仍能复制成功
 *   3. clipboard 被拒（权限/非安全上下文返回 rejected）同样回退
 *   4. 两条路都失败时必须给失败提示，绝不能报成功
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

vi.mock('element-plus', () => ({
  ElMessage: { success: vi.fn(), warning: vi.fn(), error: vi.fn(), info: vi.fn() },
}))

import { ElMessage } from 'element-plus'
import { copyText } from './clipboard'

/** 注入/移除 navigator.clipboard（只读属性，需 defineProperty） */
function setClipboard(value: unknown): void {
  Object.defineProperty(globalThis.navigator, 'clipboard', { value, configurable: true })
}

/** 注入 document.execCommand（jsdom 未实现） */
function setExecCommand(fn: (() => boolean) | undefined): void {
  Object.defineProperty(document, 'execCommand', { value: fn, configurable: true })
}

describe('copyText 复制文本', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  afterEach(() => {
    setClipboard(undefined)
    setExecCommand(undefined)
  })

  it('有 clipboard API 时走 writeText 并提示成功', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    setClipboard({ writeText })

    await expect(copyText('01 03 00 00', '帧已复制')).resolves.toBe(true)
    expect(writeText).toHaveBeenCalledWith('01 03 00 00')
    expect(ElMessage.success).toHaveBeenCalledWith('帧已复制')
    expect(ElMessage.warning).not.toHaveBeenCalled()
  })

  it('无 clipboard API（http 直连）时回退 execCommand，仍算成功', async () => {
    setClipboard(undefined)
    setExecCommand(() => true)

    await expect(copyText('现场文本')).resolves.toBe(true)
    expect(ElMessage.success).toHaveBeenCalledWith('已复制')
    expect(ElMessage.warning).not.toHaveBeenCalled()
  })

  it('clipboard 写入被拒时回退 execCommand，不能静默失败', async () => {
    setClipboard({ writeText: vi.fn().mockRejectedValue(new Error('NotAllowedError')) })
    setExecCommand(() => true)

    await expect(copyText('现场文本')).resolves.toBe(true)
    expect(ElMessage.success).toHaveBeenCalledWith('已复制')
  })

  it('两条路都失败时给失败提示并返回 false（绝不报成功）', async () => {
    setClipboard(undefined)
    setExecCommand(() => false)

    await expect(copyText('现场文本')).resolves.toBe(false)
    expect(ElMessage.warning).toHaveBeenCalledWith('复制失败，请手动选择文本')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('execCommand 直接抛错（老浏览器）同样降级为失败提示', async () => {
    setClipboard(undefined)
    setExecCommand(() => { throw new Error('unsupported') })

    await expect(copyText('现场文本')).resolves.toBe(false)
    expect(ElMessage.warning).toHaveBeenCalledWith('复制失败，请手动选择文本')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('空文本直接提示「没有可复制的内容」，不触碰剪贴板', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    setClipboard({ writeText })

    await expect(copyText('')).resolves.toBe(false)
    expect(writeText).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalledWith('没有可复制的内容')
  })
})
