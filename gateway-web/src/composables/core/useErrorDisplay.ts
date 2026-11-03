/**
 * 统一错误显示 composable
 *
 * 解决项目里错误处理风格不统一的问题：
 *   - 有时 ElMessage.error（Toast）
 *   - 有时 alert（原生弹窗）
 *   - 有时 inline error（行内提示）
 *   - 有时 console.error（静默丢失）
 *
 * 提供 3 种风格的统一入口，调用方只需关心场景（瞬时/持久/详细），不需关心实现。
 *
 * 使用示例：
 *   const { showToast, showInline } = useErrorDisplay()
 *   showToast(e, '读取点位失败')         // 弹 Toast
 *   showInline(error.value, e)          // 行内 + Toast（行内绑定到响应式变量）
 */
import { ElButton, ElMessage, ElMessageBox } from 'element-plus'
import { h } from 'vue'
import { extractErrorMessage } from '@/utils/errorHandler'
import { withDiagHint } from '@/utils/diagHints'
import { logError } from '@/utils/logger'
import { copyText } from '@/utils/clipboard'

export type ErrorStyle = 'toast' | 'inline' | 'modal' | 'silent'

export interface UseErrorDisplay {
  /** 弹 Toast 提示（最常用，用于瞬时错误） */
  showToast: (err: unknown, fallback?: string) => void
  /** 行内提示 + 同时 Toast（用于表单提交失败需要展示位置） */
  showInline: (inlineRef: { value: string | null }, err: unknown, fallback?: string) => void
  /** 弹模态确认框（用于严重错误需要用户确认） */
  showModal: (err: unknown, title?: string) => Promise<unknown>
  /**
   * 弹「可复制」错误框：错误正文 + 一键复制按钮。
   *
   * 用于导出失败等需要用户把完整错误（含上下文 / 原始响应）回传研发的场景——
   * 纯 `ElMessage.error` Toast 无法选中文本，现场只能靠截图，排查成本极高。
   *
   * @param err    错误对象
   * @param title  弹窗标题，默认「操作失败」
   * @param detail 额外上下文（点位 / 时间段 / 原始响应等），会追加到可复制正文
   */
  showCopyable: (err: unknown, title?: string, detail?: string) => Promise<unknown>
  /** 静默处理（仅 console.error，用于测试/批量场景） */
  silent: (err: unknown) => void
  /** 从 unknown 提取错误消息 */
  message: (err: unknown, fallback?: string) => string
}

export function useErrorDisplay(): UseErrorDisplay {
  // 审查#6：诊断建议原先只在 http 层全局 toast 拼接，本地 toast/inline/modal 全部没有。
  // 收敛到 message() 统一拼 withDiagHint，三种展示风格走同一文案。
  function message(err: unknown, fallback = '操作失败'): string {
    return withDiagHint(extractErrorMessage(err, fallback))
  }

  function showToast(err: unknown, fallback = '操作失败'): void {
    const msg = message(err, fallback)
    ElMessage.error(msg)
  }

  function showInline(
    inlineRef: { value: string | null },
    err: unknown,
    fallback = '操作失败',
  ): void {
    const msg = message(err, fallback)
    inlineRef.value = msg
    ElMessage.error(msg)
  }

  function showModal(err: unknown, title = '错误'): Promise<unknown> {
    return ElMessageBox.alert(message(err), title, { type: 'error' })
  }

  function showCopyable(err: unknown, title = '操作失败', detail?: string): Promise<unknown> {
    const msg = detail ? `${message(err)}\n\n${detail}` : message(err)
    return ElMessageBox.alert(
      h(
        'div',
        { style: { display: 'flex', flexDirection: 'column', gap: '10px', maxWidth: '560px' } },
        [
          h(
            'div',
            {
              style: {
                whiteSpace: 'pre-wrap',
                wordBreak: 'break-all',
                maxHeight: '260px',
                overflow: 'auto',
                textAlign: 'left',
                lineHeight: '1.6',
              },
            },
            msg,
          ),
          h(
            ElButton,
            { size: 'small', type: 'primary', onClick: () => void copyText(msg, '错误信息已复制') },
            () => '复制错误信息',
          ),
        ],
      ),
      title,
      { type: 'error', confirmButtonText: '关闭', showCancelButton: false },
    )
  }

  function silent(err: unknown): void {
    logError('err', err, '[silent]')
  }

  return { showToast, showInline, showModal, showCopyable, silent, message }
}