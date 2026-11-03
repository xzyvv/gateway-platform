import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 守护测试：语义色当「文字色」时必须用 --text-* 令牌，不能用原色。
 *
 * 背景：--color-success(#67c23a) / --color-warning(#e6a23c) / --color-danger(#f56c6c)
 * / --color-info(#909399) 直接当文字色时，在白底上只有 2.04~3.08:1，
 * 而 WCAG AA 对正文要求 4.5:1 —— 越是要看清的告警（错误提示、失败数）越模糊。
 * 对应的深色文字变体 --text-success/-warning/-danger/-info 已满足 AA。
 *
 * 但原色并非一律禁用：图标、边框、进度条填充、背景色用原色是正确做法
 * （图形元素不适用文字对比度要求，且原色更醒目）。因此本测试只拦截
 * 「文字色」误用原色的情况，图标等场景通过白名单放行。
 */

/** 允许使用原色的场景：图标 / 装饰性图形 / 边框 */
const ALLOWED_SELECTOR_HINTS = [
  'icon',        // *-icon / .el-icon —— 图标非文字，不适用文字对比度
  'fav-btn',     // 收藏星标按钮（图标按钮）
  'update-close',// 关闭按钮的 × 符号
  'surge-up', 'surge-down', // 涨跌箭头图标
  'dot',         // 状态圆点（纯色块）
  'sum-ic',      // 概览汇总卡图标（图标着色）
  'check',       // 对勾图标（StepDebug 步骤结果）
  '.ic',         // 状态图标（SlaveScan 扫描结果格）
]

/**
 * .ts 文件中已知的非文字色 --el-color-* 用法（放行白名单）。
 * 这些函数返回值用于 ElProgress stroke / ElTag background 等图形场景，
 * 不适用文字对比度要求；若新增文字色用途必须改用 --text-*。
 */
const TS_NONTEXT_ALLOWLIST: string[] = [
  // 断言 ElProgress 的 color 属性（进度条填充色），属图形用途
  'ConfigHealth.spec.ts',
]

/** 原色 → 对应的合规文字令牌 */
const COLOR_TO_TEXT: Record<string, string> = {
  success: '--text-success',
  warning: '--text-warning',
  danger: '--text-danger',
  info: '--text-info',
}

const here = dirname(fileURLToPath(import.meta.url))
const srcRoot = resolve(here, '../..')

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    let st
    try {
      st = statSync(p)
    } catch {
      continue
    }
    if (st.isDirectory()) walk(p, out)
    else if (/\.(css|vue|ts)$/.test(name)) out.push(p)
  }
  return out
}

/**
 * 找出「用原色当文字色」的违规点。
 *
 * 判定方式：扫描所有 `color: var(--color-xxx)` 出现处，回溯定位它所属的
 * 选择器块（支持单行内联块 `.x { color: ... }` 与多行块两种写法），
 * 若该块的选择器命中白名单（图标等）则放行，否则记为违规。
 */
function findViolations(): string[] {
  const violations: string[] = []
  // 匹配「纯粹的 color 属性」：前面不能是 -border-/-background-/text- 等前缀
  // （border-left-color / border-color / background-color 用原色是正确的）
  const COLOR_RE = /(?<![\w-])color:\s*var\(--color-(success|warning|danger|info)\)/g

  for (const file of walk(srcRoot)) {
    const src = readFileSync(file, 'utf8')
    const rel = `src/${relative(srcRoot, file).split('\\').join('/')}`
    const lines = src.split(/\r?\n/)

    // .ts 文件：扫描 `var(--el-color-(success|warning|danger|info))` 字符串字面量。
    // TS 中无法静态判定返回值用于 color 还是 background，故除白名单外一律视为违规
    // （文字色必须用 --text-*；图形用途需显式登记到 TS_NONTEXT_ALLOWLIST）。
    if (rel.endsWith('.ts')) {
      const TS_RE = /var\(--el-color-(success|warning|danger|info)\)/g
      const basename = rel.split('/').pop()!
      const allowed = TS_NONTEXT_ALLOWLIST.some((f) => basename === f)
      if (allowed) continue
      lines.forEach((line, idx) => {
        TS_RE.lastIndex = 0
        let m: RegExpExecArray | null
        while ((m = TS_RE.exec(line)) !== null) {
          const kind = m[1]!
          violations.push(
            `${rel}:${idx + 1}  文字色误用 var(--el-color-${kind})  → 应用 var(${COLOR_TO_TEXT[kind]})（图形用途请登记 TS_NONTEXT_ALLOWLIST）`,
          )
        }
      })
      continue
    }

    // CSS/Vue：--el-color-* 语义原色当文字色同样不达 AA（原色白底仅 2.04~3.08:1），
    // 必须改用 --text-* 变体。图形场景（图标/进度条/背景）用原色是正确的。
    const EL_COLOR_RE = /(?<![\w-])color:\s*var\(--el-color-(success|warning|danger|info)\)/g

    lines.forEach((line, idx) => {
      EL_COLOR_RE.lastIndex = 0
      let m: RegExpExecArray | null
      while ((m = EL_COLOR_RE.exec(line)) !== null) {
        const kind = m[1]!

        // .vue 模板里的 inline style：无法回溯到选择器。
        // 模板行里出现 <el-icon 说明染的是图标，不是文字。
        if (rel.endsWith('.vue') && line.includes('style=')) {
          if (line.includes('<el-icon') || line.includes('<svg')) continue
        }

        // 渲染函数里的图标着色（h(ElIcon, { style: 'color:...' })）：
        // ElIcon 是图标组件，图形元素不适用文字对比度要求。
        if (line.includes('ElIcon')) continue

        // 回溯选择器（支持单行内联块与多行块两种写法）
        let selector = ''
        const before = line.slice(0, m.index)
        const inlineSel = before.match(/([^{}]+)\{[^}]*$/)
        if (inlineSel) {
          selector = inlineSel[1]!
        } else {
          for (let j = idx; j >= 0 && j > idx - 8; j--) {
            const l = lines[j]!
            if (l.includes('{') && !l.trim().startsWith('/*') && !l.trim().startsWith('*')) {
              selector = l
              break
            }
          }
        }
        // 找不到选择器说明是全局规则（如 :root 内的令牌定义）——放行
        if (!selector) continue

        const allowed = ALLOWED_SELECTOR_HINTS.some((hint) => selector.includes(hint))
        if (allowed) continue

        violations.push(
          `${rel}:${idx + 1}  ${selector.trim()}  → 应用 var(${COLOR_TO_TEXT[kind]})`,
        )
      }
    })

    lines.forEach((line, idx) => {
      COLOR_RE.lastIndex = 0
      let m: RegExpExecArray | null
      while ((m = COLOR_RE.exec(line)) !== null) {
        const kind = m[1]!

        // .vue 模板里的 inline style（style="..."）：无法回溯到选择器，
        // 只能按上下文人工放行。已知合法场景是图标着色（el-icon 内）。
        // 模板行里出现 <el-icon 说明染的是图标，不是文字。
        if (rel.endsWith('.vue') && line.includes('style=')) {
          if (line.includes('<el-icon') || line.includes('<svg')) continue
        }

        // 单行内联块：选择器与声明在同一行（形如 `.x { color: ... }`）
        let selector = ''
        const before = line.slice(0, m.index)
        const inlineSel = before.match(/([^{}]+)\{[^}]*$/)
        if (inlineSel) {
          selector = inlineSel[1]!
        } else {
          // 多行块：往上找最近的选择器行（最多回溯 8 行，覆盖嵌套规则）
          for (let j = idx; j >= 0 && j > idx - 8; j--) {
            const l = lines[j]!
            if (l.includes('{') && !l.trim().startsWith('/*') && !l.trim().startsWith('*')) {
              selector = l
              break
            }
          }
        }
        // 找不到选择器说明是全局规则（如 :root 内的令牌定义）——放行
        if (!selector) continue

        const allowed = ALLOWED_SELECTOR_HINTS.some((hint) => selector.includes(hint))
        if (allowed) continue

        violations.push(
          `${rel}:${idx + 1}  ${selector.trim()}  → 应用 var(${COLOR_TO_TEXT[kind]})`,
        )
      }
    })
  }
  return violations
}

describe('语义色文字令牌守护', () => {
  it('--text-success/-warning/-danger/-info 四个文字令牌均已定义', () => {
    const css = readFileSync(resolve(srcRoot, 'assets/styles/global.css'), 'utf8')
    for (const token of ['--text-success', '--text-warning', '--text-danger', '--text-info']) {
      expect(css, `global.css 缺少 ${token} 令牌定义`).toContain(`${token}:`)
    }
  })

  it('暗色模式为 --text-* 令牌提供了覆盖（避免暗底下用浅色系的浅色文字）', () => {
    const css = readFileSync(resolve(srcRoot, 'assets/styles/global.css'), 'utf8')
    const darkBlock = css.slice(css.indexOf('html.dark'))
    for (const token of ['--text-success', '--text-warning', '--text-danger', '--text-info']) {
      expect(darkBlock, `html.dark 缺少 ${token} 覆盖`).toContain(`${token}:`)
    }
  })

  it('语义色不得直接用作文字色（应改用 --text-* 令牌）', () => {
    const violations = findViolations()
    expect(
      violations,
      `以下位置把语义原色当文字色用，对比度不达 WCAG AA：\n${violations.join('\n')}`,
    ).toEqual([])
  })
})
