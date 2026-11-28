/**
 * 帧文本（HEX）的规范化与合法性判断。
 *
 * DD-47：具体实现已收敛到 `hexCodec.ts`（全项目单一真源）。
 * 本文件保留为兼容层 —— 对外符号不变，调用方 import 路径无需修改。
 *
 * 报文编辑、单步调试、抓包预填三个页签都要判断「这段 HEX 能不能直接下发到总线」。
 * 放在 utils 而不是某个 composable 里，是因为这是纯函数、无状态：
 * 一旦各处各自实现，就会出现「编辑器放行、单步拦截」这类不一致，
 * 而现场表现为「同样的帧一个页面能发、另一个页面发不出」。
 */

export { normalizeHex, isValidHex, isWriteFrame } from './hexCodec'
