import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 守护测试：圆角必须落在 --radius-* 令牌体系内。
 *
 * 背景：圆角是全站高频样式（改造前 210 处硬编码字面量），散落的
 * `border-radius: 10px` 与 `var(--radius-card)` 指向同一视觉值却有两个来源，
 * 调整设计规范时必然漏改。VISUAL_SPEC §4 已给出唯一映射：
 *   10px → --radius-card    8px → --radius-table
 *    6px → --radius-button  4px → --radius-small
 *    2px → --radius-bar
 *
 * 令牌值与原字面量完全一致（10/8/6/4/2px），因此本改造是零视觉差异的
 * 纯来源归口，不改变任何渲染结果。
 *
 * 放行项：
 *   - 50%：圆形（头像/圆点/进度环），不是像素圆角
 *   - 无令牌覆盖的长尾值（3/5/7/9/12/16/20/999px）：一次性语义化样式，
 *     抽象收益为负（见 project_memory「阴影令牌化若存在大量长尾…抽象收益为负」）
 *   - !important 覆盖：EP 组件内部样式优先级考量，改动风险大于收益
 *   - 多值简写（如 `0 6px 6px 0`）：单角场景，无对应令牌
 */

/** 有令牌的圆角字面量 → 令牌名 */
const RADIUS_TOKENS: Record<string, string> = {
  '10px': '--radius-card',
  '8px': '--radius-table',
  '6px': '--radius-button',
  '4px': '--radius-small',
  '2px': '--radius-bar',
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
    else if (/\.(css|vue)$/.test(name)) out.push(p)
  }
  return out
}

/** 找出仍在使用圆角字面量（非令牌）的位置 */
function findViolations(): string[] {
  const violations: string[] = []
  const RADIUS_RE = /border-radius:\s*([^;}]+)/g

  for (const file of walk(srcRoot)) {
    const src = readFileSync(file, 'utf8')
    const rel = `src/${relative(srcRoot, file).split('\\').join('/')}`
    const lines = src.split(/\r?\n/)

    lines.forEach((line, idx) => {
      RADIUS_RE.lastIndex = 0
      let m: RegExpExecArray | null
      while ((m = RADIUS_RE.exec(line)) !== null) {
        const value = m[1]!.trim()

        // !important 覆盖放行
        if (value.includes('!important')) continue
        // 多值简写（0 6px 6px 0 / var(...) var(...) 0 0）放行：单角场景无对应令牌
        if (/\s/.test(value)) continue
        // 已是令牌放行
        if (value.startsWith('var(')) continue
        // 50% 圆形放行
        if (value === '50%') continue
        // inherit / 0 等非尺寸值放行
        if (value === 'inherit' || value === '0') continue
        // 长尾值（无令牌覆盖）放行
        if (!RADIUS_TOKENS[value]) continue

        violations.push(
          `${rel}:${idx + 1}  border-radius: ${value}  → 应用 var(${RADIUS_TOKENS[value]})`,
        )
      }
    })
  }
  return violations
}

describe('圆角令牌守护', () => {
  it('--radius-* 令牌均已在 global.css 定义', () => {
    const css = readFileSync(resolve(srcRoot, 'assets/styles/global.css'), 'utf8')
    for (const token of Object.values(RADIUS_TOKENS)) {
      expect(css, `global.css 缺少 ${token} 令牌定义`).toContain(`${token}:`)
    }
  })

  it('圆角不得使用字面量（应改用 --radius-* 令牌）', () => {
    const violations = findViolations()
    expect(
      violations,
      `以下位置仍在用圆角字面量，散落的来源会导致设计规范调整时漏改：\n${violations.join('\n')}`,
    ).toEqual([])
  })
})
