import { describe, it, expect } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 守护测试：语义文字色在明暗两种主题下都必须满足 WCAG AA 正文对比度 4.5:1。
 *
 * 背景：文字色从 --el-color-* / --color-* 换成 --text-* 之后，明暗两套值不同
 * （暗色下切到提亮的 -light 变体），而文字常常落在 --color-*-bg 这类
 * 半透明语义底色上——底色不同，实际对比度也不同，只看单一底色会漏判。
 *
 * 本测试直接从 global.css 解析出「明暗两套实际生效值」，
 * 对每个文字色 × 每种可能底色（卡底/填充底/页面底/卡头底/主色底/四种语义底色）逐一验算。
 * 任一组合低于 4.5:1 即失败——宁可调色也不放行。
 */

const here = dirname(fileURLToPath(import.meta.url))
const css = readFileSync(resolve(here, 'global.css'), 'utf8')

/** 取出从 startIdx 开始的完整 CSS 块（含配对花括号） */
function sliceBlock(src: string, startIdx: number): string {
  let depth = 0
  for (let i = startIdx; i < src.length; i++) {
    if (src[i] === '{') depth++
    else if (src[i] === '}') {
      depth--
      if (depth === 0) return src.slice(startIdx, i + 1)
    }
  }
  return src.slice(startIdx)
}

/** 解析块内 CSS 变量定义（值可能是 hex / var() / rgba()） */
function parseVars(block: string): Map<string, string> {
  const map = new Map<string, string>()
  const re = /--([a-z0-9-]+)\s*:\s*([^;]+);/g
  let m: RegExpExecArray | null
  while ((m = re.exec(block)) !== null) map.set(m[1]!, m[2]!.trim())
  return map
}

function normalizeHex(hex: string): string {
  const h = hex.replace('#', '')
  if (h.length === 3) return '#' + h.split('').map((c) => c + c).join('')
  if (h.length === 8) return '#' + h.slice(0, 6)
  return '#' + h
}

/** 解析变量链 → { hex, alpha }；支持 var() 嵌套、hex、rgb()/rgba() */
function resolveVar(
  vars: Map<string, string>,
  name: string,
  seen = new Set<string>(),
  depth = 0,
): { hex: string; alpha: number } | null {
  if (depth > 12 || seen.has(name)) return null
  seen.add(name)
  const raw = vars.get(name)
  if (!raw) return null
  const vm = raw.match(/var\(\s*--([a-z0-9-]+)\s*\)/)
  if (vm) return resolveVar(vars, vm[1]!, seen, depth + 1)
  const rm = raw.match(/rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+))?\s*\)/)
  if (rm) {
    const hex = '#' + [rm[1], rm[2], rm[3]]
      .map((v) => Number(v).toString(16).padStart(2, '0'))
      .join('')
    return { hex, alpha: rm[4] !== undefined ? parseFloat(rm[4]) : 1 }
  }
  const hm = raw.match(/^(#[0-9a-fA-F]{3,8})$/)
  if (hm) return { hex: normalizeHex(hm[1]!.toLowerCase()), alpha: 1 }
  return null
}

function hexOf(vars: Map<string, string>, name: string): string | null {
  return resolveVar(vars, name)?.hex ?? null
}
function alphaOf(vars: Map<string, string>, name: string): number | null {
  return resolveVar(vars, name)?.alpha ?? null
}

/** 前景色以 alpha 叠加到背景色上（暗色下 --color-*-bg 是半透明 rgba） */
function composite(fgHex: string, alpha: number, bgHex: string): string {
  const toRgb = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16))
  const fg = toRgb(fgHex)
  const bg = toRgb(bgHex)
  const out = fg.map((c, i) => Math.round(c * alpha + bg[i]! * (1 - alpha)))
  return '#' + out.map((v) => v.toString(16).padStart(2, '0')).join('')
}

function luminance(hex: string): number {
  const rgb = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255)
  const lin = rgb.map((s) => (s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4)))
  return 0.2126 * lin[0]! + 0.7152 * lin[1]! + 0.0722 * lin[2]!
}

function contrast(fg: string, bg: string): number {
  const l1 = luminance(fg)
  const l2 = luminance(bg)
  const [hi, lo] = l1 > l2 ? [l1, l2] : [l2, l1]
  return (hi + 0.05) / (lo + 0.05)
}

// ---- 解析明暗两套变量 ----
const lightVars = parseVars(sliceBlock(css, css.indexOf(':root {')))
const darkVars = parseVars(sliceBlock(css, css.indexOf('html.dark {')))
// 暗色块只重声明部分变量，其余继承 :root —— 合并后才是暗色实际生效值
const mergedDark = new Map([...lightVars, ...darkVars])

/** 暗色下把半透明语义底色合成到卡底上（暗色语义底是 rgba(...)，不是实色） */
function darkBottom(name: string): string | null {
  const hex = hexOf(mergedDark, name)
  const alpha = alphaOf(mergedDark, name)
  if (!hex) return null
  if (alpha === null || alpha === 1) return hex
  const card = hexOf(mergedDark, 'bg-card')!
  return composite(hex, alpha, card)
}

/** 文字色 → 需要核对的底色清单
 *
 *  覆盖正文三级（primary/regular/secondary）+ placeholder + 四种语义文字色。
 *  不含 --text-disabled：WCAG 1.4.3 明确豁免禁用控件的文字。
 *  不含 --color-mark-bg 作底色：搜索高亮 mark 的文字用 --color-mark-text 单独配对（见下方专项断言）。 */
const TEXT_COLORS = [
  'text-primary',
  'text-regular',
  'text-secondary',
  'text-placeholder',
  'text-success',
  'text-warning',
  'text-danger',
  'text-info',
] as const

/** 所有可能的底色（元素可能落在这些背景上） */
const BOTTOM_NAMES = [
  'bg-card',
  'bg-fill',
  'bg-page',
  'bg-card-header',
  'color-success-bg',
  'color-warning-bg',
  'color-danger-bg',
  'color-info-bg',
  'color-primary-bg',
] as const

const AA = 4.5

function checkTheme(label: string, vars: Map<string, string>, bottomOf: (n: string) => string | null) {
  const failures: string[] = []
  for (const fgName of TEXT_COLORS) {
    const fg = hexOf(vars, fgName)
    expect(fg, `${label} 缺少 ${fgName} 的可解析色值`).toBeTruthy()
    for (const bgName of BOTTOM_NAMES) {
      const bg = bottomOf(bgName)
      if (!bg) continue
      const c = contrast(fg!, bg)
      if (c < AA) {
        failures.push(`${fgName}(${fg}) on ${bgName}(${bg}) = ${c.toFixed(2)}:1 < ${AA}`)
      }
    }
  }
  return failures
}

describe('语义文字色对比度守护（WCAG AA 4.5:1）', () => {
  it('亮色主题：文字色在所有底色上均达 AA', () => {
    const failures = checkTheme('亮色', lightVars, (n) => hexOf(lightVars, n))
    expect(failures, `亮色主题对比度不达 AA：\n${failures.join('\n')}`).toEqual([])
  })

  it('暗色主题：文字色在所有底色上均达 AA（半透明语义底已合成到卡底）', () => {
    const failures = checkTheme('暗色', mergedDark, darkBottom)
    expect(failures, `暗色主题对比度不达 AA：\n${failures.join('\n')}`).toEqual([])
  })

  it('暗色主题的语义文字色不是直接沿用亮色值（否则暗底上不可读）', () => {
    for (const fgName of TEXT_COLORS) {
      const light = hexOf(lightVars, fgName)
      const dark = hexOf(mergedDark, fgName)
      expect(dark, `暗色缺少 ${fgName}`).toBeTruthy()
      expect(
        light === dark,
        `${fgName} 明暗同值 ${light}：暗底上对比度必然不足`,
      ).toBe(false)
    }
  })

  it('搜索高亮 mark：--color-mark-text 落在 --color-mark-bg 上达 AA（明暗同值）', () => {
    for (const [label, vars] of [['亮色', lightVars], ['暗色', mergedDark]] as const) {
      const fg = hexOf(vars, 'color-mark-text')
      const bg = hexOf(vars, 'color-mark-bg')
      expect(fg, `${label} 缺少 --color-mark-text`).toBeTruthy()
      expect(bg, `${label} 缺少 --color-mark-bg`).toBeTruthy()
      const c = contrast(fg!, bg!)
      expect(c, `${label} mark 文字对比度 ${c.toFixed(2)}:1 < ${AA}`).toBeGreaterThanOrEqual(AA)
    }
  })
})
