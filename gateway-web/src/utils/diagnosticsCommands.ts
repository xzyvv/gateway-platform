/**
 * 诊断命令白名单 + 前端预校验
 *
 * B2 修复：诊断 exec 涉及"在网关执行 shell 命令"——后端虽有严格白名单
 *（见 [gateway-core/src/engine/monitor/remote_diag.rs] 的 `validate_whitelist`），
 *前端仍做一层**预校验**，提供即时反馈、降低 4xx 调用频次、保护 UX。
 *
 * 关键设计：
 * 1. **白名单来源**：`ALLOWED_PROGRAMS` 与后端 `ALLOWED` 常量保持一致；
 *    如后端增删命令，需要同步更新此处。**前端预校验不能放行后端禁用的命令**——
 *    反之后端**仍会**拒（最终安全屏障），前端只是 UX 优化。
 * 2. **预校验内容**：program 在白名单内、参数不含危险字符（`|`、`>`、`<`、`;`、
 *    `&`、`` ` ``、`$`、`*`、反引号）、总长度不超过 4 KB（与后端 `MAX_CMD_LEN` 对齐）。
 * 3. **预校验结果**：返回结构化 `{ ok, error?, programHint? }`——UI 据此给红字提示。
 * 4. **快捷命令**：与 useLogs 历史 `QUICK_COMMANDS` 等价（已被替换），保留后端白名单
 *    子集，避免后端拒绝的快捷按钮看起来能点。
 */

const ALLOWED_PROGRAMS: ReadonlySet<string> = new Set([
  'df',
  'free',
  'uptime',
  'top',
  'ps',
  'dmesg',
  'ip',
  'uname',
  'date',
  'ping',
  'netstat',
  'ss',
  'lsof',
  'iostat',
  'vmstat',
  'du',
  'journalctl',
  'systemctl',
  'gpio',
  'modbus',
  'serial',
])

/** 命令最大长度（与后端 MAX_CMD_LEN 对齐） */
const MAX_CMD_LEN = 4096

/** 与后端 `validate_whitelist` 的危险字符列表保持一致 */
const FORBIDDEN_CHARS = ['|', '>', '<', ';', '&', '`', '$', '*']

/** 预校验结果 */
export interface PrecheckResult {
  ok: boolean
  /** 错误时的中文/英文提示 */
  error?: string
  /** 提取出的 program 名（用于 UI 展示） */
  program?: string
}

/** 预校验一条命令字符串：返回 ok 或 error，不抛异常 */
export function precheckCommand(cmd: string): PrecheckResult {
  const trimmed = cmd.trim()
  if (!trimmed) {
    return { ok: false, error: '命令不能为空' }
  }
  if (trimmed.length > MAX_CMD_LEN) {
    return {
      ok: false,
      error: `命令长度 ${trimmed.length} 超过上限 ${MAX_CMD_LEN}`,
    }
  }
  // 危险字符：任一参数含即拒（与后端语义一致）
  const parts = trimmed.split(/\s+/)
  for (let i = 1; i < parts.length; i++) {
    const arg = parts[i] ?? ''
    for (const ch of FORBIDDEN_CHARS) {
      if (arg.includes(ch)) {
        return {
          ok: false,
          error: `参数含禁用字符 '${ch}'（仅允许只读诊断命令）`,
        }
      }
    }
  }
  const program = parts[0] ?? ''
  // basename：允许 `/usr/bin/df` 等带路径，但最终只接受 ALLOWED 内
  const base = program.split('/').pop() ?? program
  if (!ALLOWED_PROGRAMS.has(base)) {
    return {
      ok: false,
      error: `命令 '${base}' 不在白名单（允许: ${Array.from(ALLOWED_PROGRAMS).join(', ')}）`,
    }
  }
  return { ok: true, program: base }
}

/** 常用诊断命令（与后端白名单子集对齐——仅 precheckCommand 通过的项） */
export const QUICK_DIAGNOSTIC_COMMANDS: ReadonlyArray<string> = [
  'df -h',
  'free -m',
  'uptime',
  'uname -a',
  'ip addr',
  'ping -c 4 8.8.8.8',
  'ps aux',
  'ss -tlnp',
  'journalctl -n 50 --no-pager',
  'systemctl status gateway-core',
  'du -sh /var/log/gateway',
  'vmstat 1 3',
  'iostat -xz 1 2',
  'date',
  'netstat -tlnp',
]
