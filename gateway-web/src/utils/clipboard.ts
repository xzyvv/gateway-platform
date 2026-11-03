import { ElMessage } from 'element-plus'

/**
 * 复制纯文本到剪贴板，并给出统一的成功/失败反馈。
 *
 * 为什么不用 `navigator.clipboard` 直写：
 * 现场网关常用 `http://<ip>:<port>` 直接访问，属于非安全上下文，
 * 多数浏览器此时不提供 `navigator.clipboard`（或返回 rejected）。
 * 通信帧排查恰恰依赖把原文贴给研发，所以这里保留 textarea + execCommand 的回退路径。
 *
 * @param text     要复制的文本
 * @param okMessage 成功提示，默认「已复制」
 * @returns 是否复制成功（失败时已提示用户，调用方无需再弹错）
 */
export async function copyText(text: string, okMessage = '已复制'): Promise<boolean> {
  if (!text) {
    ElMessage.warning('没有可复制的内容')
    return false
  }
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text)
      ElMessage.success(okMessage)
      return true
    }
  } catch {
    /* 非安全上下文或权限被拒：走下面的回退 */
  }
  try {
    const ta = document.createElement('textarea')
    ta.value = text
    ta.setAttribute('readonly', '')
    ta.style.position = 'fixed'
    ta.style.top = '-9999px'
    ta.style.opacity = '0'
    document.body.appendChild(ta)
    ta.select()
    const ok = document.execCommand('copy')
    document.body.removeChild(ta)
    if (ok) {
      ElMessage.success(okMessage)
      return true
    }
  } catch {
    /* 回退也失败：统一走下面的失败提示 */
  }
  ElMessage.warning('复制失败，请手动选择文本')
  return false
}
