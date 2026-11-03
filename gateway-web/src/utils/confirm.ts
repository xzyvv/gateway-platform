import { ElMessageBox } from 'element-plus'

interface ConfirmOptions {
  /** 必填：确认提示文字（如 "确定删除该设备吗？"） */
  message: string
  /** 对话框标题，默认 "操作确认" */
  title?: string
  /** 图标类型，默认 "warning" */
  type?: 'warning' | 'info' | 'success' | 'error'
  /** 确认按钮文字，默认 "确定" */
  confirmText?: string
  /** 取消按钮文字，默认 "取消" */
  cancelText?: string
}

/**
 * 操作确认对话框（统一封装 ElMessageBox.confirm）
 *
 * 统一 21 处 "await ElMessageBox.confirm(msg, title, {type:'warning'})" 的重复模式。
 * 与原 API 的关键差异：返回 Promise<boolean>，取消时返回 false 不抛错，
 * 调用方无需 try/catch 即可处理取消分支。
 *
 * 示例：
 *   if (await confirmAction({ message: '确定退出登录？' })) {
 *     auth.logout()
 *   }
 *
 *   if (await confirmAction({ message: '套用模板将覆盖当前配置？', title: '套用模板' })) {
 *     applyTemplate()
 *   }
 */
export async function confirmAction(opts: ConfirmOptions): Promise<boolean> {
  try {
    await ElMessageBox.confirm(opts.message, opts.title ?? '操作确认', {
      type: opts.type ?? 'warning',
      confirmButtonText: opts.confirmText ?? '确定',
      cancelButtonText: opts.cancelText ?? '取消',
    })
    return true
  } catch {
    // 用户点取消/关闭对话框
    return false
  }
}

interface DeleteOptions {
  /** 对话框标题，默认 "删除确认" */
  title?: string
  /** 图标类型，默认 "warning" */
  type?: 'warning' | 'error'
  /** 确认按钮文字，默认 "删除" */
  confirmText?: string
  /** 取消按钮文字，默认 "取消" */
  cancelText?: string
  /**
   * 删除后仍可撤销的窗口（秒）。
   * 传了就提示「N 秒内可撤销」，不传才是「不可恢复」——
   * 文案必须与实际行为一致，否则用户会怀疑通知里的「撤销删除」是不是假的。
   */
  undoSeconds?: number
}

/**
 * 删除确认对话框（confirmAction 的删除场景便捷封装）
 *
 * 自动拼装 "确定删除「<name>」？删除后不可恢复" 文案（传 undoSeconds 时改为可撤销），
 * 约 80% 的确认调用都是删除场景。
 *
 * 示例：
 *   if (await confirmDelete(device.name)) {
 *     await deleteDevice(device.id)
 *     ElMessage.success('已删除')
 *   }
 *
 *   if (await confirmDelete(channel.name, { confirmText: '禁用' })) {
 *     await disableChannel(channel.id)
 *   }
 */
export function confirmDelete(name: string, opts: DeleteOptions = {}): Promise<boolean> {
  const tail = opts.undoSeconds
    ? `删除后 ${opts.undoSeconds} 秒内可在通知中撤销`
    : '删除后不可恢复'
  return confirmAction({
    message: `确定删除「${name}」？${tail}`,
    title: opts.title ?? '删除确认',
    type: opts.type ?? 'warning',
    confirmText: opts.confirmText ?? '删除',
    cancelText: opts.cancelText ?? '取消',
  })
}