/**
 * B2 修复单测：诊断命令预校验
 *
 * 覆盖：
 * 1. 空白 / 长度超限 拒绝
 * 2. 危险字符拒绝（与后端 FORBIDDEN_CHARS 列表一致）
 * 3. 非白名单 program 拒绝
 * 4. 白名单 program + 干净 args 通过
 * 5. 快捷命令列表全部通过 precheck（避免"点了按钮但被拒"的 UX 撕裂）
 */
import { describe, it, expect } from 'vitest'
import {
  precheckCommand,
  QUICK_DIAGNOSTIC_COMMANDS,
} from './diagnosticsCommands'

describe('precheckCommand', () => {
  it('空命令被拒', () => {
    expect(precheckCommand('').ok).toBe(false)
    expect(precheckCommand('   ').ok).toBe(false)
  })

  it('长度超限被拒', () => {
    const longCmd = 'df ' + 'a'.repeat(5000)
    const r = precheckCommand(longCmd)
    expect(r.ok).toBe(false)
    expect(r.error).toMatch(/长度/)
  })

  it.each([
    'df -h | grep /dev',
    'ps aux > /tmp/out',
    'free < /proc/meminfo',
    'uptime; reboot',
    'uname -a & whoami',
    'echo `id`',
    'echo $(id)',
    'rm -rf /*',
  ])('危险字符拒绝: %s', (cmd) => {
    const r = precheckCommand(cmd)
    expect(r.ok).toBe(false)
  })

  it.each(['rm', 'mv', 'dd', 'chmod', 'kill', 'reboot', 'shutdown', 'curl', 'wget', 'cat', 'ls', 'mount'])
    ('非白名单 program 拒绝: %s', (program) => {
      const r = precheckCommand(`${program} -h`)
      expect(r.ok).toBe(false)
      expect(r.error).toMatch(/白名单/)
    })

  it('白名单 program 通过', () => {
    expect(precheckCommand('df -h').ok).toBe(true)
    expect(precheckCommand('free -m').ok).toBe(true)
    expect(precheckCommand('uptime').ok).toBe(true)
    expect(precheckCommand('ps aux').ok).toBe(true)
    expect(precheckCommand('ip addr').ok).toBe(true)
  })

  it('白名单 program 拼带路径前缀也可（与后端 validate_whitelist 的 basename 校验一致）', () => {
    expect(precheckCommand('/usr/bin/df -h').ok).toBe(true)
    expect(precheckCommand('/bin/free -m').ok).toBe(true)
  })

  it('非空 ok 时返回 program basename', () => {
    const r = precheckCommand('df -h')
    expect(r.ok).toBe(true)
    expect(r.program).toBe('df')
  })
})

describe('QUICK_DIAGNOSTIC_COMMANDS', () => {
  it('所有快捷命令都通过 precheckCommand', () => {
    for (const cmd of QUICK_DIAGNOSTIC_COMMANDS) {
      const r = precheckCommand(cmd)
      expect(r.ok, `快捷命令 ${cmd} 未通过预校验: ${r.error}`).toBe(true)
    }
  })

  it('列表非空且不含重复', () => {
    expect(QUICK_DIAGNOSTIC_COMMANDS.length).toBeGreaterThan(5)
    expect(new Set(QUICK_DIAGNOSTIC_COMMANDS).size).toBe(QUICK_DIAGNOSTIC_COMMANDS.length)
  })
})
