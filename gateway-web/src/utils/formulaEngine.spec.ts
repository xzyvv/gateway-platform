import { describe, it, expect } from 'vitest'
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

  // ===== 与 Rust `core::pipeline::vformula` 的跨语言一致性 =====
  // Rust 侧是权威实现；以下用例必须与 `gateway-core/src/pipeline/vformula/tests.rs`
  // 表达同一语义。改动任一侧都必须同步另一侧。
  describe('与 Rust vformula 语义一致', () => {
    it('幂运算右结合', () => {
      expect(evaluateFormula('2^3^2', {})).toBe(512)
    })

    it('LOG 为自然对数（非 2 为底）', () => {
      expect(evaluateFormula('LOG(8)', {})).toBeCloseTo(2.0794415, 6)
    })

    it('ROUND 保留指定小数位；省略时等价于 0 位', () => {
      expect(evaluateFormula('ROUND(12.34567, 2)', {})).toBe(12.35)
      expect(evaluateFormula('ROUND(12.34567)', {})).toBe(12)
    })

    it('ROUND 的半值朝 +∞ 取整（与 JS Math.round 一致）', () => {
      expect(evaluateFormula('ROUND(2.5)', {})).toBe(3)
      expect(evaluateFormula('ROUND(-2.5)', {})).toBe(-2)
    })

    it('IF 条件按非零为真（NaN 为假）', () => {
      expect(evaluateFormula('IF(1, 10, 20)', {})).toBe(10)
      expect(evaluateFormula('IF(0, 10, 20)', {})).toBe(20)
    })

    it('AVG / MAX / MIN 为变参', () => {
      expect(evaluateFormula('AVG(1, 9, 5)', {})).toBe(5)
      expect(evaluateFormula('MAX(1, 9, 5)', {})).toBe(9)
      expect(evaluateFormula('MIN(1, 9, 5)', {})).toBe(1)
    })

    it('取模与除法正常求值', () => {
      expect(evaluateFormula('7 % 3', {})).toBe(1)
      expect(evaluateFormula('7 / 2', {})).toBe(3.5)
    })

    it('常量 PI / E 可用', () => {
      expect(evaluateFormula('PI', {})).toBeCloseTo(Math.PI, 6)
      expect(evaluateFormula('E', {})).toBeCloseTo(Math.E, 6)
    })

    it('`.Value` 后缀大小写不敏感剥离', () => {
      expect(evaluateFormula('{a.Value} * 2', { a: 3 })).toBe(6)
      expect(evaluateFormula('{a.value} * 2', { a: 3 })).toBe(6)
    })

    it('运算符优先级与左结合（与 Rust 一致）', () => {
      expect(evaluateFormula('1 + 2 * 3', {})).toBe(7)
      expect(evaluateFormula('(1 + 2) * 3', {})).toBe(9)
      expect(evaluateFormula('10 - 4 - 3', {})).toBe(3)
      expect(evaluateFormula('10 / 4', {})).toBe(2.5)
      expect(evaluateFormula('-(2+3)', {})).toBe(-5)
    })

    it('负数取模的结果与 Rust 同号（朝零截断）', () => {
      expect(evaluateFormula('-7 % 3', {})).toBe(-1)
    })

    it('结果归一到 6 位小数', () => {
      expect(evaluateFormula('1/3', {})).toBe(0.333333)
    })

    it('引用提取覆盖花括号与裸标识符', () => {
      expect(extractPointRefs('{a} + {b.Value} * 2')).toEqual(['a', 'b'])
    })

    it('非法字符 / 非法数字 / 花括号不配对均被判非法（对齐 Rust 词法层）', () => {
      expect(validateFormula('2 @ + 3').valid).toBe(false)
      expect(validateFormula('1.2.3 + 1').valid).toBe(false)
      expect(validateFormula('{a + 1').valid).toBe(false)
      expect(validateFormula('{} + 1').valid).toBe(false)
    })
  })
})
