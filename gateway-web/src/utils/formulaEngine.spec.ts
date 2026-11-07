import { describe, it, expect } from 'vitest'
import { existsSync, readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { validateFormula, evaluateFormula, extractPointRefs } from './formulaEngine'

describe('formulaEngine', () => {
  // 修复(审查#15) 回归：语法校验不应依赖点位是否有实时值
  it('引用点位的合法公式必须通过语法校验', () => {
    expect(validateFormula('{dev1.temp} * 2 + 1').valid).toBe(true)
    expect(validateFormula('ROUND({a} / {b}, 2)').valid).toBe(true)
    expect(validateFormula('100 * 2').valid).toBe(true)
  })

  it('语法错误仍然被拦截', () => {
    expect(validateFormula('').valid).toBe(false)
    expect(validateFormula('{a} *').valid).toBe(false)
    expect(validateFormula('{a} + 1)').valid).toBe(false)
    expect(validateFormula('FOO(1)').valid).toBe(false)
  })

  it('求值路径仍严格要求点位有值', () => {
    expect(evaluateFormula('{a} * 2', { a: 21 })).toBe(42)
    expect(() => evaluateFormula('{missing} * 2', {})).toThrow()
  })
})

/**
 * 跨语言用例契约（O-7）
 *
 * 用例集在仓库根 `shared/formula_cases.json`，Rust 侧
 * `gateway-core/src/pipeline/vformula/tests.rs` 读的是**同一个文件**。
 * 语义要改就改这份契约——只改单侧，另一侧会立刻转红。
 *
 * 错误类别的对应关系（前端没有 Rust 那样的错误枚举）：
 *   syntax  → `validateFormula().valid === false`（词法/语法层拦截）
 *   eval    → `evaluateFormula` 抛错（除零、非有限结果）
 *   missing → `evaluateFormula` 抛错（引用点位无值）
 */
interface SharedCase {
  name: string
  expr: string
  args?: Record<string, number>
  expected?: number
  tolerance?: number
  refs?: string[]
  error?: 'syntax' | 'eval' | 'missing'
}

// vmThreads 环境下 `import.meta.url` 不是 file:// 协议，无法用 fileURLToPath；
// 改为从进程工作目录逐级向上找 `shared/formula_cases.json`（vitest 的 cwd 是 gateway-web）。
function findCasesFile(): string {
  let dir = process.cwd()
  for (let i = 0; i < 6; i++) {
    const candidate = join(dir, 'shared', 'formula_cases.json')
    if (existsSync(candidate)) return candidate
    const parent = dirname(dir)
    if (parent === dir) break
    dir = parent
  }
  throw new Error('找不到仓库根的 shared/formula_cases.json')
}

const sharedCases: SharedCase[] = JSON.parse(readFileSync(findCasesFile(), 'utf8')).cases

describe('与 Rust vformula 的共享用例契约（shared/formula_cases.json）', () => {
  it('用例集非空', () => {
    expect(sharedCases.length).toBeGreaterThan(0)
  })

  for (const c of sharedCases) {
    it(c.name, () => {
      const args = c.args ?? {}

      if (c.error === 'syntax') {
        expect(validateFormula(c.expr).valid, `期望语法错误：${c.expr}`).toBe(false)
        return
      }
      if (c.error === 'eval' || c.error === 'missing') {
        expect(() => evaluateFormula(c.expr, args), `期望抛错：${c.expr}`).toThrow()
        return
      }
      if (c.refs) {
        expect(extractPointRefs(c.expr)).toEqual(c.refs)
        return
      }

      if (c.expected === undefined) throw new Error(`用例「${c.name}」缺少 expected 字段`)
      const tolerance = c.tolerance ?? 1e-6
      const actual = evaluateFormula(c.expr, args)
      expect(Math.abs(actual - c.expected), `期望 ${c.expected}，实际 ${actual}：${c.expr}`)
        .toBeLessThanOrEqual(tolerance)
    })
  }
})
