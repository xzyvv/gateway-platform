/**
 * 帧文本（HEX）的规范化与合法性判断。
 *
 * 报文编辑、单步调试、抓包预填三个页签都要判断「这段 HEX 能不能直接下发到总线」。
 * 放在 utils 而不是某个 composable 里，是因为这是纯函数、无状态：
 * 一旦各处各自实现，就会出现「编辑器放行、单步拦截」这类不一致，
 * 而现场表现为「同样的帧一个页面能发、另一个页面发不出」。
 */

/** 展示/发送统一格式：去首尾空白、连续空白压成一个空格、转大写 */
export function normalizeHex(input: string): string {
  return input.trim().replace(/\s+/g, ' ').toUpperCase()
}

/**
 * 合法帧：去掉空格后为偶数长度且仅含 0-9A-F。
 *
 * 空串视为不合法 —— 调用方据此禁用「发送」按钮；奇数字节（如 `01 3`）
 * 和含非十六进制字符（如 `ZZ`）都必须在本地拦下，
 * 不能把半截帧或乱码下发到真实总线（轻则返回异常码，重则干扰在跑的采集）。
 */
export function isValidHex(input: string): boolean {
  const compact = normalizeHex(input).replace(/ /g, '')
  return compact.length > 0 && compact.length % 2 === 0 && /^[0-9A-F]+$/.test(compact)
}
