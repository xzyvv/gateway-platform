import { describe, it, expect } from 'vitest'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

/**
 * 守护测试：表单控件必须有可访问名称（aria-label 或关联的 <label>）。
 *
 * 背景：读屏器用户听到的只是「编辑框」「组合框」，不知道是哪个字段——
 * 现场排查时无法定位「是哪个输入框没填」。placeholder 只是提示文字，
 * 不是可访问名称（读屏器不会播报它，且部分实现会忽略）。
 *
 * 判定「已有名称」的两种方式：
 *   1. 控件自身带 aria-label（Element Plus 会透传到真实可聚焦的 <input>）
 *   2. 被 <el-form-item label="..."> 包裹 —— EP 会生成 <label for> 关联
 *
 * 放行项：
 *   - el-switch / el-radio / el-checkbox 等非文本输入控件：EP 内置 role+label
 */

const here = dirname(fileURLToPath(import.meta.url))
const srcRoot = resolve(here, '..')

/** 需要名称的表单控件 */
const TEXT_INPUT_COMPONENTS = ['el-input', 'el-select', 'el-input-number'] as const

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
    else if (/\.vue$/.test(name)) out.push(p)
  }
  return out
}

/** 取出 <template> 段（Vue SFC 的第一个 template 块） */
function getTemplate(src: string): string {
  const s = src.indexOf('<template>')
  const e = src.lastIndexOf('</template>')
  return s >= 0 && e > s ? src.slice(s, e) : ''
}

interface UnnamedControl {
  file: string
  line: number
  tag: string
  snippet: string
}

/**
 * 找出没有可访问名称的表单控件。
 *
 * 难点：属性可跨多行、控件可嵌套在 <el-form-item> 内（该情况下 label 关联成立）。
 * 做法：按标签扫描 template，对每个控件：
 *   - 自身有 aria-label → 通过
 *   - 处于某个带 label= 的 <el-form-item> 未闭合区间内 → 通过
 *   - 否则记为违规
 */
function findUnnamedControls(): UnnamedControl[] {
  const violations: UnnamedControl[] = []

  for (const file of walk(srcRoot)) {
    const rel = `src/${relative(srcRoot, file).split('\\').join('/')}`

    const src = readFileSync(file, 'utf8')
    const tpl = getTemplate(src)
    if (!tpl) continue

    const re = new RegExp(`<(${TEXT_INPUT_COMPONENTS.join('|')})\\b([^>]*?)(?:\\/>|>)`, 'g')
    let m: RegExpExecArray | null
    while ((m = re.exec(tpl)) !== null) {
      const attrs = m[2] || ''
      if (/aria-label/.test(attrs)) continue

      // 往前找最近的 <el-form-item ...>，判断它是否带 label 且尚未闭合
      const before = tpl.slice(0, m.index)
      const items = [...before.matchAll(/<el-form-item\b([^>]*)>/g)]
      const last = items[items.length - 1]
      const lastAttrs = last?.[1] ?? ''
      if (last && /label=/.test(lastAttrs)) {
        const after = before.slice(last.index)
        const closed = (after.match(/<\/el-form-item>/g) || []).length
        const opened = (after.match(/<el-form-item\b/g) || []).length
        if (closed < opened) continue
      }

      const line = tpl.slice(0, m.index).split(/\r?\n/).length
      violations.push({
        file: rel,
        line,
        tag: m[1]!,
        snippet: attrs.replace(/\s+/g, ' ').trim().slice(0, 90),
      })
    }
  }

  return violations
}

describe('表单控件可访问名称守护', () => {
  it('扫描到了表单控件（防止正则失效导致本测试假通过）', () => {
    const all = walk(srcRoot)
    const withControls = all.filter((f) => {
      const tpl = getTemplate(readFileSync(f, 'utf8'))
      return new RegExp(`<(${TEXT_INPUT_COMPONENTS.join('|')})\\b`).test(tpl)
    })
    expect(withControls.length, '未扫描到任何表单控件，测试可能已失效').toBeGreaterThan(20)
  })

  it('每个表单控件都有可访问名称（aria-label 或 el-form-item label）', () => {
    const violations = findUnnamedControls()
    expect(
      violations,
      '以下表单控件缺少可访问名称，读屏器只会播报「编辑框」而不知道是哪个字段：\n' +
        violations.map((v) => `  ${v.file}:${v.line}  <${v.tag}>  ${v.snippet}`).join('\n'),
    ).toEqual([])
  })
})
