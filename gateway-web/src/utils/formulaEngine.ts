/**
 * 点位计算公式引擎（浏览器端预演 / 语法校验）
 * 支持：四则运算、括号、函数（ABS/ROUND/MAX/MIN/SQRT/POW/AVG/CEIL/FLOOR/LOG/EXP/IF）、点位引用 {point_id}
 * 使用状态机分词 + 递归下降解析，避免 eval 安全风险
 *
 * # 权威语义在 Rust 侧
 *
 * 本文件只是**浏览器端预演**（保存前校验、公式编辑提示），
 * **权威实现是 `gateway-core/src/pipeline/vformula.rs`**——虚拟点位的真实求值
 * 由网关的 Rust 求值器执行。两者的语义必须保持一致：
 *
 * | 语义点 | 本文件 | Rust `vformula` |
 * |---|---|---|
 * | `LOG` | 自然对数 `ln` | `ln` |
 * | `^` | 右结合（`2^3^2 = 512`） | 右结合 |
 * | `ROUND(x, n)` | 保留 n 位小数 | 保留 n 位小数 |
 * | `ROUND(x)` | 等价 `ROUND(x, 0)` | 同 |
 * | `IF(cond, a, b)` | 条件非零为真（NaN 为假） | 同 |
 * | `AVG/MAX/MIN` | 变参 | 变参 |
 * | `.Value` 后缀 | 大小写不敏感剥离 | 同 |
 * | 结果精度 | 归一到 6 位小数 | 同 |
 *
 * 修改本文件时必须同步修改 Rust 侧，并补充两侧的一致性用例。
 */

import { extractErrorMessage } from './errorHandler'

export type TokenType = 'number' | 'op' | 'lp' | 'rp' | 'comma' | 'func' | 'ident' | 'eof'

export interface Token { type: TokenType; value: string }

const FUNCS = ['ABS', 'ROUND', 'MAX', 'MIN', 'SQRT', 'POW', 'AVG', 'CEIL', 'FLOOR', 'LOG', 'EXP', 'IF']
const CONSTS: Record<string, number> = { PI: Math.PI, E: Math.E }

/** 词法分析 */
export function tokenize(expr: string): Token[] {
  const tokens: Token[] = []
  let i = 0
  const s = expr.replace(/\s+/g, ' ')
  while (i < s.length) {
    const c = s[i]!
    if (c === ' ') { i++; continue }
    if (c === '(') { tokens.push({ type: 'lp', value: '(' }); i++; continue }
    if (c === ')') { tokens.push({ type: 'rp', value: ')' }); i++; continue }
    if (c === ',') { tokens.push({ type: 'comma', value: ',' }); i++; continue }
    if ('+-*/%^'.includes(c)) { tokens.push({ type: 'op', value: c }); i++; continue }
    // 数字
    if (/[0-9.]/.test(c)) {
      let num = ''
      while (i < s.length && /[0-9.]/.test(s[i]!)) { num += s[i]!; i++ }
      tokens.push({ type: 'number', value: num })
      continue
    }
    // 标识符（函数名/常量/点位引用）
    if (/[a-zA-Z_]/.test(c) || c === '{') {
      let ident = ''
      if (c === '{') {
        // 点位引用 {point_id} 或 {point_id.Value}
        let j = i + 1
        while (j < s.length && s[j] !== '}') { ident += s[j]!; j++ }
        if (j < s.length) j++ // skip }
        tokens.push({ type: 'ident', value: `{${ident}}` })
        i = j
        continue
      }
      while (i < s.length && /[a-zA-Z0-9_]/.test(s[i]!)) { ident += s[i]!; i++ }
      const upper = ident.toUpperCase()
      if (FUNCS.includes(upper)) tokens.push({ type: 'func', value: upper })
      else if (CONSTS[upper] !== undefined) tokens.push({ type: 'number', value: String(CONSTS[upper]!) })
      else tokens.push({ type: 'ident', value: ident })
      continue
    }
    i++
  }
  tokens.push({ type: 'eof', value: '' })
  return tokens
}

/** 递归下降解析器 */
class Parser {
  pos = 0
  tokens: Token[]
  pointValues: Record<string, number>
  /** 仅校验语法（不关心取值）：点位缺少运行值时以 0 占位而不报错 */
  validateOnly: boolean
  constructor(tokens: Token[], pointValues: Record<string, number>, validateOnly = false) {
    this.tokens = tokens
    this.pointValues = pointValues
    this.validateOnly = validateOnly
  }

  peek(): Token { return this.tokens[this.pos]! }
  consume(): Token { return this.tokens[this.pos++]! }

  parseExpression(): number {
    let left = this.parseTerm()
    while (this.peek().type === 'op' && (this.peek().value === '+' || this.peek().value === '-')) {
      const op = this.consume().value
      const right = this.parseTerm()
      left = op === '+' ? left + right : left - right
    }
    return left
  }

  parseTerm(): number {
    let left = this.parsePower()
    while (this.peek().type === 'op' && '*/%'.includes(this.peek().value)) {
      const op = this.consume().value
      const right = this.parsePower()
      if (op === '*') left = left * right
      else if (op === '/') left = right === 0 ? NaN : left / right
      else left = left % right
    }
    return left
  }

  /** 幂运算 `^`，右结合：2^3^2 = 2^(3^2) = 512 */
  parsePower(): number {
    const base = this.parseFactor()
    if (this.peek().type === 'op' && this.peek().value === '^') {
      this.consume()
      const exp = this.parsePower()
      return Math.pow(base, exp)
    }
    return base
  }

  parseFactor(): number {
    const tok = this.peek()
    if (tok.type === 'op' && tok.value === '-') {
      this.consume()
      return -this.parseFactor()
    }
    if (tok.type === 'op' && tok.value === '+') {
      this.consume()
      return this.parseFactor()
    }
    if (tok.type === 'number') {
      this.consume()
      return parseFloat(tok.value)
    }
    if (tok.type === 'lp') {
      this.consume()
      const val = this.parseExpression()
      if (this.peek().type !== 'rp') throw new Error('缺少右括号')
      this.consume()
      return val
    }
    if (tok.type === 'func') {
      return this.parseFuncCall()
    }
    if (tok.type === 'ident') {
      this.consume()
      // 点位引用 {xxx} 或 {xxx.Value}
      const inner = tok.value.replace(/[{}]/g, '').replace(/\.Value$/i, '')
      const v = this.pointValues[inner]
      // 修复(审查#15)：点位有无运行值与"公式语法是否正确"无关。校验阶段点位必然没有实时值，
      // 若在此抛错会让所有引用点位的合法公式都被判为非法。
      if (v === undefined) {
        if (this.validateOnly) return 0
        throw new Error(`点位 "${inner}" 未找到或无值`)
      }
      return v
    }
    throw new Error(`意外的 token: ${tok.value}`)
  }

  parseFuncCall(): number {
    const name = this.consume().value
    if (this.peek().type !== 'lp') throw new Error(`函数 ${name} 后缺少左括号`)
    this.consume()
    const args: number[] = []
    if (this.peek().type !== 'rp') {
      args.push(this.parseExpression())
      while (this.peek().type === 'comma') {
        this.consume()
        args.push(this.parseExpression())
      }
    }
    if (this.peek().type !== 'rp') throw new Error(`函数 ${name} 参数列表缺少右括号`)
    this.consume()
    switch (name) {
      case 'ABS': return Math.abs(args[0]!)
      case 'ROUND': return Math.round(args[0]! * Math.pow(10, args[1]! || 0)) / Math.pow(10, args[1]! || 0)
      case 'MAX': return Math.max(...args)
      case 'MIN': return Math.min(...args)
      case 'SQRT': return Math.sqrt(args[0]!)
      case 'POW': return Math.pow(args[0]!, args[1]!)
      case 'AVG': return args.reduce((a, b) => a + b, 0) / args.length
      case 'CEIL': return Math.ceil(args[0]!)
      case 'FLOOR': return Math.floor(args[0]!)
      case 'LOG': return Math.log(args[0]!)
      case 'EXP': return Math.exp(args[0]!)
      case 'IF': return args[0] ? args[1]! : args[2]!
      default: throw new Error(`不支持的函数: ${name}`)
    }
  }
}

/** 计算公式 */
export function evaluateFormula(formula: string, pointValues: Record<string, number>): number {
  // 与 Rust 求值器一致：词法层错误直接拒绝，不做容错跳过
  const issue = lexIssue(formula)
  if (issue) throw new Error(issue)
  const tokens = tokenize(formula)
  const parser = new Parser(tokens, pointValues)
  const result = parser.parseExpression()
  if (parser.peek().type !== 'eof') throw new Error('公式末尾有多余内容')
  if (!isFinite(result)) throw new Error('计算结果无效（除以零或溢出）')
  return Math.round(result * 1e6) / 1e6
}

/**
 * 提取公式中引用的点位 ID
 *
 * 与 Rust `vformula::CompiledFormula::refs()` 口径一致：**裸标识符**同样视为
 * 点位引用（Rust 求值器把未知标识符一律当点位引用），因此这里收集全部
 * `ident` 记号（花括号引用与裸标识符都算）。
 */
export function extractPointRefs(formula: string): string[] {
  const refs = new Set<string>()
  const tokens = tokenize(formula)
  for (const t of tokens) {
    if (t.type === 'ident') {
      const inner = t.value.replace(/[{}]/g, '').replace(/\.Value$/i, '').trim()
      if (inner) refs.add(inner)
    }
  }
  return [...refs]
}

/**
 * 词法层问题检查（对齐 Rust `vformula::lex` 的错误语义）
 *
 * Rust 侧对非法字符、非法数字、点位引用缺右花括号 / 引用为空一律报语法错误；
 * 本文件的 `tokenize` 为容错会静默跳过它们，导致两侧判定不一致
 * （例：`2 @ + 3` 在浏览器端被判为合法 = `5`，网关侧报错）。
 * 这里补一道显式检查，使两侧判定一致。
 */
const ILLEGAL_CHAR_RE = /[^0-9a-zA-Z_\s.+\-*/%^(){},]/

/** 返回词法层错误描述；无问题返回 null */
function lexIssue(formula: string): string | null {
  const bad = formula.match(ILLEGAL_CHAR_RE)
  if (bad) return `公式含非法字符 "${bad[0]}"`
  // 数字记号必须能被完整解析（拒绝 "1.2.3" 这类多小数点写法）
  for (const t of tokenize(formula)) {
    if (t.type === 'number' && !Number.isFinite(Number(t.value))) {
      return `公式含非法数字 "${t.value}"`
    }
  }
  // 花括号必须配对且引用非空
  const opens = (formula.match(/\{/g) || []).length
  const closes = (formula.match(/\}/g) || []).length
  if (opens !== closes) return '点位引用缺少配对的右花括号 "}"'
  if (/\{\s*\}/.test(formula)) return '点位引用为空'
  return null
}

/** 校验公式语法（不计算） */
export function validateFormula(formula: string): { valid: boolean; error?: string } {
  try {
    const issue = lexIssue(formula)
    if (issue) return { valid: false, error: issue }
    const tokens = tokenize(formula)
    if (tokens.length <= 1) return { valid: false, error: '公式为空' }
    // 修复(审查#15)：第三个参数 validateOnly=true。原实现复用求值用的 Parser 并传空点位表，
    // 使任何引用点位 {xxx} 的合法公式都在 parseFactor 抛"点位未找到或无值" → 校验恒为失败，
    // 界面上"语法正确"永不出现、表达式转换规则永远无法保存。
    const parser = new Parser(tokens, {}, true)
    parser.parseExpression()
    if (parser.peek().type !== 'eof') return { valid: false, error: '公式末尾有多余内容' }
    return { valid: true }
  } catch (e) {
    return { valid: false, error: extractErrorMessage(e, '语法错误') }
  }
}
