import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

/**
 * 守护 src/utils/ep-icons-compact.ts 里所有 SVG path 的语法合法性。
 *
 * 背景：该文件由 scripts/_gen_compact_icons.mjs 生成。旧版生成器用
 * /\d+\.\d+/g 直接对 path 做正则替换取整，会把「相邻且省略分隔符的两个数」
 * （如 `-5.376.32` = `-5.376` 与 `.32`）粘成一个数，导致 cubic 参数少一个、
 * 整条 path 语法非法，浏览器抛 `<path> attribute d: Expected number`。
 * 这类错误不会让构建失败，只会在运行时静默毁掉图标，因此必须单测兜住。
 */

// 直接读取生成后的源码文本：图标被 ic() 包成组件，path 数据不在导出的
// 组件对象上，只有源文件里能拿到原始字符串。
const SRC = readFileSync(resolve(process.cwd(), 'src/utils/ep-icons-compact.ts'), 'utf8')

/** 各命令字母所需的参数个数（大小写同义） */
const ARG_COUNT: Record<string, number> = {
  M: 2, L: 2, H: 1, V: 1, C: 6, S: 4, Q: 4, T: 2, A: 7, Z: 0,
}

/** 按 SVG path 语法校验一条 d，返回错误描述；合法返回 null */
function validatePath(d: string): string | null {
  let cmd: string | null = null
  let filled = 0
  let i = 0
  while (i < d.length) {
    const ch = d[i]!
    if (ch === ' ' || ch === ',' || ch === '\t' || ch === '\n' || ch === '\r') {
      i++
      continue
    }
    if (/[A-Za-z]/.test(ch)) {
      if (cmd !== null && filled !== 0) {
        const need = ARG_COUNT[cmd.toUpperCase()]
        return `命令 ${cmd} 参数不足（还差 ${(need ?? 0) - filled} 个）却被 "${ch}" 截断`
      }
      if (ARG_COUNT[ch.toUpperCase()] === undefined) return `未知命令 "${ch}"`
      cmd = ch
      filled = 0
      i++
      continue
    }
    if (cmd === null) return `命令字母前出现数字（位置 ${i}）`
    const m = /^[+-]?(?:\d+\.?\d*|\.\d+)/.exec(d.slice(i))
    if (!m) return `位置 ${i} 处不是数字：…${d.slice(Math.max(0, i - 12), i + 12)}…`
    i += m[0].length
    filled++
    const need = ARG_COUNT[cmd.toUpperCase()]!
    if (filled === need) {
      filled = 0
      // 首个大写 M 之后的坐标对按 L 处理（SVG 规范）
      if (cmd === 'M') cmd = 'L'
      else if (cmd === 'm') cmd = 'l'
      else if (need === 0) cmd = null
    }
  }
  if (cmd !== null && filled !== 0) {
    const need = ARG_COUNT[cmd.toUpperCase()]!
    return `命令 ${cmd} 结束时参数不足（还差 ${need - filled} 个）`
  }
  return null
}

/** 从生成文件里取出所有图标的 path 列表 */
function extractIcons(): { name: string; paths: string[] }[] {
  const icons: { name: string; paths: string[] }[] = []
  const re = /export const ([A-Za-z0-9_]+) = ic\((\[[^\]]*\]|"[^"]*")\)/g
  let m: RegExpExecArray | null
  while ((m = re.exec(SRC)) !== null) {
    const parsed = JSON.parse(m[2]!) as string | string[]
    icons.push({ name: m[1]!, paths: Array.isArray(parsed) ? parsed : [parsed] })
  }
  return icons
}

describe('ep-icons-compact 生成产物', () => {
  const icons = extractIcons()

  it('能解析出全部图标', () => {
    expect(icons.length).toBeGreaterThan(200)
  })

  it('所有 path 均符合 SVG path 语法', () => {
    const bad: string[] = []
    for (const { name, paths } of icons) {
      paths.forEach((p, idx) => {
        const err = validatePath(p)
        if (err) bad.push(`${name} path#${idx}: ${err}\n    ${p}`)
      })
    }
    expect(bad).toEqual([])
  })

  it('path 里的每个数都是整数（取整不产生小数点）', () => {
    const offenders: string[] = []
    for (const { name, paths } of icons) {
      paths.forEach((p, idx) => {
        if (/\d\.\d/.test(p)) offenders.push(`${name} path#${idx}: ${p.slice(0, 80)}`)
      })
    }
    expect(offenders).toEqual([])
  })

  it('回归：UploadFilled 的 cubic 参数没有被粘连', () => {
    const icon = icons.find((i) => i.name === 'UploadFilled')
    expect(icon).toBeTruthy()
    // 旧 bug 会把 `c-5.376.32-10.496 1.6-16 1.6`（6 个参数）压成 5 个
    expect(validatePath(icon!.paths[0]!)).toBeNull()
    expect(icon!.paths[0]).toContain('A240 240')
  })
})
