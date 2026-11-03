/**
 * 导入类页面的 JSON 文本校验（R1：校验口径统一）。
 *
 * 「导入导出」「片段导入」等入口此前各自写一份 `JSON.parse` + 报错文案，
 * 同一段坏 JSON 在不同页面给出的提示不一致，现场人员无法判断是不是同一个问题。
 * 这里把「解析 + 中文原因」收成一处：页面只负责把结果渲染到输入框下方。
 */
import type { FormItemRule } from 'element-plus'
import { extractErrorMessage } from './errorHandler'

/** 解析结果：失败时 error 为可直接展示给现场人员的中文原因 */
export interface JsonTextParseResult<T = unknown> {
  ok: boolean
  value?: T
  error?: string
}

/**
 * 解析 JSON 文本，失败不抛异常。
 *
 * @param text 待解析文本（允许前后空白）
 * @param label 字段名，用于拼出「XX不能为空 / XX格式错误」这类可定位的提示
 */
export function parseJsonText<T = unknown>(text: unknown, label = 'JSON'): JsonTextParseResult<T> {
  const raw = String(text ?? '').trim()
  if (!raw) return { ok: false, error: `${label}不能为空` }
  try {
    return { ok: true, value: JSON.parse(raw) as T }
  } catch (e) {
    return { ok: false, error: `${label}格式错误，无法解析：${extractErrorMessage(e)}` }
  }
}

/**
 * 生成 Element Plus 表单 validator。
 *
 * 解析失败时把原因写到输入框下方（字段级），而不是只弹一条汇总 toast ——
 * 现场改配置时，能直接看到是哪段 JSON 坏了。
 */
export function makeJsonTextValidator(label: string): FormItemRule['validator'] {
  return (_rule, value, callback) => {
    const result = parseJsonText(value, label)
    if (!result.ok) callback(new Error(result.error))
    else callback()
  }
}
