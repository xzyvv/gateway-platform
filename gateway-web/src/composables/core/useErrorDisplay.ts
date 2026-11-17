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
 * # 第二个参数是「动作短语」，不是「兜底文案」
 *
 * 历史上这里叫 `fallback`，而 `extractErrorMessage` 的 fallback 只在抛出非 Error 时
 * 生效 —— 于是 68 处 `showToast(e, '加载转换规则失败')` 在实际出错（Error 实例）时
 * 只显示了后端原因，动作语境被丢掉。现在第二个参数统一按**动作短语**处理，输出
 * 「<动作>：<原因>」（拼装规则见 `utils/errorHandler.ts` 的 `composeFailure`）。
 *
 * 使用示例：
 *   const { showToast, showInline } = useErrorDisplay()
 *   showToast(e, '读取点位失败')         // 「读取点位失败：<原因>」
 *   showInline(errorRef, e, '保存点位失败')
 */

import { ElButton, ElMessage, ElMessageBox } from 'element-plus'
import { h } from 'vue'
import { composeFailure, extractErrorMessage, isUserNotified } from '@/utils/errorHandler'
import { withDiagHint } from '@/utils/diagHints'
import { logError } from '@/utils/logger'
import { copyText } from '@/utils/clipboard'

export type ErrorStyle = 'toast' | 'inline' | 'modal' | 'silent'

/** 未给动作语境时的默认动作短语 */
const DEFAULT_ACTION = '操作失败'

export interface UseErrorDisplay {
  /**
   * 弹 Toast 提示（最常用，用于瞬时错误）。
   *
   * 若该错误已由 HTTP 层弹过全局通知（GET 失败），本方法**不再重复弹**——
   * 同一错误两条提示对现场只是噪音。需要保留页面内反馈时用 `showInline`。
   *
   * @param err    错误对象
   * @param action 动作短语（含「失败」二字，如「保存点位失败」）
   */
  showToast: (err: unknown, action?: string) => void
  /**
   * 行内提示 + 同时 Toast（用于表单提交失败需要展示位置）。
   *
   * 与 `showToast` 的差别：无论全局是否已提示，**行内文案一定写入**（页面靠它渲染
   * 错误条），只有 Toast 部分会去重。
   */
  showInline: (inlineRef: { value: string | null }, err: unknown, action?: string) => void
  /** 弹模态确认框（用于严重错误需要用户确认；标题即动作语境） */
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
  /** 错误文案（含动作语境）：`<action>：<原因>`；不传 action 时只回原因 */
  message: (err: unknown, action?: string) => string
}

export function useErrorDisplay(): UseErrorDisplay {
  // 审查#6：诊断建议原先只在 http 层全局 toast 拼接，本地 toast/inline/modal 全部没有。
  // 收敛到 message() 统一拼 withDiagHint，三种展示风格走同一文案。
  function message(err: unknown, action?: string): string {
    // 先补诊断建议再拼动作：建议是针对「原因」的，贴在动作前面会读成
    // 「保存失败（串口被占用）：port busy」这种因果倒置的句子。
    const reason = withDiagHint(extractErrorMessage(err, '').trim())
    if (!action) return reason || DEFAULT_ACTION
    return composeFailure(action, reason)
  }

  function showToast(err: unknown, action?: string): void {
    // 全局层（HTTP 拦截器）已经弹过同一条错误的通知时不再重复弹——
    // 典型场景：页面 onMounted 里 load() 失败，全局一条 + 本地一条。
    if (isUserNotified(err)) return
    ElMessage.error(message(err, action))
  }

  function showInline(
    inlineRef: { value: string | null },
    err: unknown,
    action?: string,
  ): void {
    const msg = message(err, action)
    inlineRef.value = msg
    if (isUserNotified(err)) return
    ElMessage.error(msg)
  }

  function showModal(err: unknown, title = '错误'): Promise<unknown> {
    // 标题承担动作语境，正文只放原因，避免标题与正文重复
    return ElMessageBox.alert(withDiagHint(extractErrorMessage(err, '')), title, { type: 'error' })
  }

  function showCopyable(err: unknown, title = DEFAULT_ACTION, detail?: string): Promise<unknown> {
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
