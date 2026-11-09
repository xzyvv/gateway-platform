import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 守护测试：`var(--x)` 引用的自定义属性必须在仓库内有定义。
 *
 * 背景：`var(--border-color)` 这类未定义令牌不会报错，而是在计算值阶段让
 * 整条声明失效（边框消失、底色丢失），且只在暗色主题或不常访问的页面上
 * 才看得出差异，极易长期潜伏。本轮清理前有 8 处此类引用（border-color /
 * bg-secondary / bg-muted / bg-card-hover / fill-color-light / radius-sm /
 * font-mono 等），全部收敛到 global.css 的真实令牌。
 *
 * 定义来源：`src` 下所有 `.css` / `.vue` 中的 `--token: value` 声明
 * （含组件内 `:root` 与 `<style>` 块）。
 *
 * 放行项：`--el-*`（Element Plus 在运行时注入，仓库内不声明）。
 */
const EXEMPT_PREFIXES = ['--el-']

const DEFINITION_RE = /(--[a-z0-9-]+)\s*:/g
const USAGE_RE = /var\(\s*(--[a-z0-9-]+)/g

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

const files = walk(srcRoot)

/** 仓库内已定义的令牌集合 */
function collectDefined(): Set<string> {
  const defined = new Set<string>()
  for (const file of files) {
    for (const m of readFileSync(file, 'utf8').matchAll(DEFINITION_RE)) {
      defined.add(m[1]!)
    }
  }
  return defined
}

/** 找出引用了未定义令牌的位置 */
function findUndefined(): string[] {
  const defined = collectDefined()
  const violations: string[] = []

  for (const file of files) {
    const rel = `src/${relative(srcRoot, file).split('\\').join('/')}`
    readFileSync(file, 'utf8')
      .split(/\r?\n/)
      .forEach((line, idx) => {
        for (const m of line.matchAll(USAGE_RE)) {
          const token = m[1]!
          if (defined.has(token)) continue
          if (EXEMPT_PREFIXES.some((p) => token.startsWith(p))) continue
          violations.push(`${rel}:${idx + 1}  var(${token}) 未在仓库内定义`)
        }
      })
  }
  return violations
}

describe('CSS 令牌守护', () => {
  it('所有 var(--x) 引用的令牌都必须有定义', () => {
    const violations = findUndefined()
    expect(
      violations,
      `以下位置引用了未定义的令牌（声明会整条失效，边框/底色/字体静默丢失）：\n${violations.join('\n')}`,
    ).toEqual([])
  })

  it('等宽字体统一走 --font-mono 令牌', () => {
    const css = readFileSync(resolve(srcRoot, 'assets/styles/global.css'), 'utf8')
    expect(css).toContain('--font-mono:')
  })
})
