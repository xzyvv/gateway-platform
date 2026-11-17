/**
 * 统一错误提示：动作语境 + 全局去重
 *
 * 这里钉两件事：
 *  1. 第二个参数是**动作短语**（不是兜底文案）。旧实现把 `extractErrorMessage` 的
 *     fallback 直接当结果，导致 68 处 `showToast(e, '加载XX失败')` 在真实出错
 *     （Error 实例）时丢掉了动作语境，只剩后端原因。
 *  2. 该错误已由 HTTP 层弹过全局通知时，页面不再叠一条 Toast（行内文案仍要写，
 *     页面靠它渲染错误条）。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'

const { alertMock } = vi.hoisted(() => ({ alertMock: vi.fn(() => Promise.resolve('confirm')) }))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  const message = Object.assign(vi.fn(), {
    error: vi.fn(),
    warning: vi.fn(),
    success: vi.fn(),
    info: vi.fn(),
  })
  return {
    ...actual,
    ElMessage: message,
    // ElMessageBox 的 alert 由 showModal / showCopyable 使用
    ElMessageBox: { ...actual.ElMessageBox, alert: alertMock },
  }
})

vi.mock('@/utils/clipboard', () => ({ copyText: vi.fn(() => Promise.resolve(true)) }))

import { ElMessage } from 'element-plus'
import { ref } from 'vue'
import { useErrorDisplay } from './useErrorDisplay'
import { markUserNotified } from '@/utils/errorHandler'

describe('useErrorDisplay', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('showToast 输出「动作：原因」', () => {
    const { showToast } = useErrorDisplay()
    showToast(new Error('总线不存在'), '启动采集失败')
    expect(ElMessage.error).toHaveBeenCalledWith('启动采集失败：总线不存在')
  })

  // 回归：旧实现此时只显示「总线不存在」，动作被丢掉
  it('原因缺失时只显示动作，不留「失败：」半截文案', () => {
    const { showToast } = useErrorDisplay()
    showToast(null, '加载转换规则失败')
    expect(ElMessage.error).toHaveBeenCalledWith('加载转换规则失败')
  })

  it('不传动作时只显示原因；原因也没有则退回「操作失败」', () => {
    const { showToast } = useErrorDisplay()
    showToast(new Error('boom'))
    expect(ElMessage.error).toHaveBeenLastCalledWith('boom')

    showToast(null)
    expect(ElMessage.error).toHaveBeenLastCalledWith('操作失败')
  })

  // 诊断建议属于「原因」，必须贴在动作之后：「动作：原因（建议）」
  it('诊断建议拼在原因后、动作之后', () => {
    const { showToast } = useErrorDisplay()
    showToast(new Error('Address already in use'), '保存总线失败')
    const shown = vi.mocked(ElMessage.error).mock.calls[0]![0] as string
    expect(shown.startsWith('保存总线失败：Address already in use')).toBe(true)
    expect(shown).toContain('端口已被占用')
  })

  it('全局层已提示过的错误，页面不再重复弹 Toast', () => {
    const err = new Error('请求失败(500): boom')
    markUserNotified(err)
    const { showToast } = useErrorDisplay()
    showToast(err, '加载设备列表失败')
    expect(ElMessage.error).not.toHaveBeenCalled()
  })

  // 行内错误条是页面唯一的错误展示位，去重只能去掉 Toast
  it('全局已提示时 showInline 仍写入行内文案，只是不弹 Toast', () => {
    const err = new Error('请求失败(500): boom')
    markUserNotified(err)
    const { showInline } = useErrorDisplay()
    const inline = ref<string | null>(null)
    showInline(inline, err, '加载质量规则失败')
    expect(inline.value).toBe('加载质量规则失败：请求失败(500): boom')
    expect(ElMessage.error).not.toHaveBeenCalled()
  })

  it('showInline 未提示过时同时写行内与 Toast', () => {
    const { showInline } = useErrorDisplay()
    const inline = ref<string | null>(null)
    showInline(inline, new Error('点位名称不能为空'), '保存点位失败')
    expect(inline.value).toBe('保存点位失败：点位名称不能为空')
    expect(ElMessage.error).toHaveBeenCalledWith('保存点位失败：点位名称不能为空')
  })

  it('message 返回带动作语境的文案（供行内/表格列直接使用）', () => {
    const { message } = useErrorDisplay()
    expect(message(new Error('解析失败 code=42'), '读取点位失败')).toBe(
      '读取点位失败：解析失败 code=42',
    )
  })

  // 标题承担动作语境，正文只放原因，避免标题与正文重复
  it('showModal 标题给语境、正文只放原因', () => {
    const { showModal } = useErrorDisplay()
    void showModal(new Error('磁盘写入失败'), '保存配置失败')
    expect(alertMock).toHaveBeenCalledWith('磁盘写入失败', '保存配置失败', { type: 'error' })
  })
})
