//! 虚拟点位公式表达式引擎（Rust 侧**权威语义**）
//!
//! 虚拟点位公式形如 `{voltage} * {current} * 0.85`，支持：
//!
//! - 四则运算与取模：`+ - * / %`
//! - 幂运算 `^`（**右结合**：`2^3^2 = 2^(3^2) = 512`）
//! - 括号、一元正负号
//! - 函数：`ABS / ROUND / MAX / MIN / SQRT / POW / AVG / CEIL / FLOOR / LOG / EXP / IF`
//! - 常量：`PI` / `E`
//! - 点位引用：`{point_id}` 或 `{point_id.Value}`（`.Value` 后缀被忽略）
//!
//! # 跨语言一致性
//!
//! 本模块是**公式语义的唯一权威实现**。前端 `gateway-web/src/utils/formulaEngine.ts`
//! 只是浏览器端预演 / 语法校验，行为必须与本模块一致（`LOG` 为自然对数、
//! `^` 右结合、`ROUND(x, n)` 保留 n 位小数、`IF` 条件按非零为真）。
//!
//! 两侧语义由**同一份用例契约**钉住：仓库根 `shared/formula_cases.json`。
//! `tests.rs` 的 `test_shared_cross_language_cases` 与前端
//! `formulaEngine.spec.ts` 读的是同一个文件——改语义请改契约，只改单侧会立刻在另一侧转红。
//!
//! # 与 `engine::formula::apply_formula` 的区别
//!
//! `apply_formula` 是**点位级**换算公式：单变量 `raw`、白名单只放行
//! `0-9 . + - * / ( )` 与 `raw`，由 `south/protocol/common/decode.rs` 消费。
//! 虚拟点位是**多变量派生**语义（可引用其它点位、可调用函数）。
//! 二者刻意不合并：合并会把点位换算的白名单边界撑破。
//!
//! # 安全性
//!
//! 解析器是自研递归下降实现，**不执行任何动态代码**；未知标识符一律
//! 当作点位引用（取不到值即报缺值错误），不存在表达式注入面。

use std::collections::HashMap;
use std::fmt;

/// 公式语义错误
#[derive(Debug, Clone, PartialEq)]
pub enum FormulaError {
    /// 词法/语法错误
    Syntax(String),
    /// 引用的点位没有可用值（未采集 / 已过期 / 质量不可用）
    MissingPoint(String),
    /// 求值错误（除零、非有限结果、参数个数错误等）
    Eval(String),
}

impl fmt::Display for FormulaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormulaError::Syntax(m) => write!(f, "公式语法错误: {}", m),
            FormulaError::MissingPoint(m) => write!(f, "引用点位不可用: {}", m),
            FormulaError::Eval(m) => write!(f, "公式求值失败: {}", m),
        }
    }
}

impl std::error::Error for FormulaError {}

/// 受支持的函数名（大写；匹配时大小写不敏感）
const FUNCTIONS: &[&str] = &[
    "ABS", "ROUND", "MAX", "MIN", "SQRT", "POW", "AVG", "CEIL", "FLOOR", "LOG", "EXP", "IF",
];

// ===== 词法 =====

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    /// 点位引用（已归一化：去掉花括号与 `.Value` 后缀）
    Ident(String),
    /// 函数名（大写）
    Func(String),
    Op(char),
    LParen,
    RParen,
    Comma,
}

/// 归一化点位引用名：去花括号 + 去尾部 `.Value`（大小写不敏感）
fn normalize_ref(raw: &str) -> String {
    let base = raw.trim().trim_start_matches('{').trim_end_matches('}');
    // `.Value` 后缀大小写不敏感（与前端 `/\.Value$/i` 一致）
    const SUFFIX: &str = ".value";
    let stripped = match base.len().checked_sub(SUFFIX.len()) {
        Some(pos) => match base.get(pos..) {
            Some(tail) if tail.eq_ignore_ascii_case(SUFFIX) => &base[..pos],
            _ => base,
        },
        None => base,
    };
    stripped.trim().to_string()
}

fn lex(src: &str) -> Result<Vec<Token>, FormulaError> {
    let s: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < s.len() {
        let c = s[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        // 数字
        if c.is_ascii_digit() || c == '.' {
            let start = i;
            while i < s.len() && (s[i].is_ascii_digit() || s[i] == '.') {
                i += 1;
            }
            let text: String = s[start..i].iter().collect();
            let v = text
                .parse::<f64>()
                .map_err(|_| FormulaError::Syntax(format!("非法数字 \"{}\"", text)))?;
            tokens.push(Token::Number(v));
            continue;
        }
        // 引用 {point_id} / {point_id.Value}
        if c == '{' {
            let start = i;
            while i < s.len() && s[i] != '}' {
                i += 1;
            }
            if i >= s.len() {
                return Err(FormulaError::Syntax("点位引用缺少右花括号 '}'".into()));
            }
            let inner: String = s[start + 1..i].iter().collect();
            i += 1; // 跳过 '}'
            let name = normalize_ref(&inner);
            if name.is_empty() {
                return Err(FormulaError::Syntax("点位引用为空".into()));
            }
            tokens.push(Token::Ident(name));
            continue;
        }
        // 标识符：函数名 / 常量 / 裸点位名
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == '_') {
                i += 1;
            }
            let text: String = s[start..i].iter().collect();
            let upper = text.to_uppercase();
            if FUNCTIONS.contains(&upper.as_str()) {
                tokens.push(Token::Func(upper));
            } else if upper == "PI" {
                tokens.push(Token::Number(std::f64::consts::PI));
            } else if upper == "E" {
                tokens.push(Token::Number(std::f64::consts::E));
            } else {
                tokens.push(Token::Ident(text));
            }
            continue;
        }
        match c {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
            }
            '+' | '-' | '*' | '/' | '%' | '^' => {
                tokens.push(Token::Op(c));
                i += 1;
            }
            other => {
                return Err(FormulaError::Syntax(format!("非法字符 '{}'", other)));
            }
        }
    }
    Ok(tokens)
}

// ===== 语法树 =====

/// 二元运算符
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
}

/// 表达式节点
#[derive(Debug, Clone, PartialEq)]
enum Expr {
    Number(f64),
    /// 引用第 `refs[slot]` 个点位
    Var(usize),
    Neg(Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
}

/// 编译后的公式：语法树 + 引用点位列表（槽位 → 点位名）
#[derive(Debug, Clone)]
pub struct CompiledFormula {
    root: Expr,
    refs: Vec<String>,
}

impl CompiledFormula {
    /// 公式引用的点位 ID（按首次出现顺序去重）
    pub fn refs(&self) -> &[String] {
        &self.refs
    }

    /// 求值：`values` 必须覆盖全部引用点位
    pub fn eval(&self, values: &HashMap<String, f64>) -> Result<f64, FormulaError> {
        let v = eval_expr(&self.root, values, &self.refs)?;
        if !v.is_finite() {
            return Err(FormulaError::Eval(format!(
                "计算结果无效（除以零或溢出）: {}",
                v
            )));
        }
        // 与前端一致：结果归一到 6 位小数，消除浮点尾差
        Ok(js_round(v * 1e6) / 1e6)
    }
}

/// 编译公式（语法检查 + 提取引用点位）
pub fn compile(src: &str) -> Result<CompiledFormula, FormulaError> {
    let tokens = lex(src)?;
    let mut p = Parser {
        tokens: &tokens,
        pos: 0,
        refs: Vec::new(),
    };
    if tokens.is_empty() {
        return Err(FormulaError::Syntax("公式为空".into()));
    }
    let root = p.parse_expr()?;
    if p.pos != tokens.len() {
        return Err(FormulaError::Syntax("公式末尾有多余内容".into()));
    }
    Ok(CompiledFormula { root, refs: p.refs })
}

/// 便捷入口：一次性编译 + 求值
pub fn apply_formula_multi(src: &str, values: &HashMap<String, f64>) -> Result<f64, FormulaError> {
    compile(src)?.eval(values)
}

// ===== 解析器 =====

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    refs: Vec<String>,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    /// 取下一个记号（返回克隆，避免与后续可变借用冲突）
    fn take(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn expect(&mut self, want: Token) -> Result<(), FormulaError> {
        match self.take() {
            Some(t) if t == want => Ok(()),
            Some(t) => Err(FormulaError::Syntax(format!(
                "期望 {:?}，实际 {:?}",
                want, t
            ))),
            None => Err(FormulaError::Syntax(format!(
                "公式意外结束，期望 {:?}",
                want
            ))),
        }
    }

    /// 记录引用点位并返回槽位
    fn slot(&mut self, name: &str) -> usize {
        if let Some(pos) = self.refs.iter().position(|r| r == name) {
            return pos;
        }
        self.refs.push(name.to_string());
        self.refs.len() - 1
    }

    // expr := term (('+' | '-') term)*
    fn parse_expr(&mut self) -> Result<Expr, FormulaError> {
        let mut left = self.parse_term()?;
        loop {
            match self.peek() {
                Some(Token::Op('+')) => {
                    self.pos += 1;
                    let right = self.parse_term()?;
                    left = Expr::Binary(BinOp::Add, Box::new(left), Box::new(right));
                }
                Some(Token::Op('-')) => {
                    self.pos += 1;
                    let right = self.parse_term()?;
                    left = Expr::Binary(BinOp::Sub, Box::new(left), Box::new(right));
                }
                _ => return Ok(left),
            }
        }
    }

    // term := power (('*' | '/' | '%') power)*
    fn parse_term(&mut self) -> Result<Expr, FormulaError> {
        let mut left = self.parse_power()?;
        loop {
            let op = match self.peek() {
                Some(Token::Op(c @ ('*' | '/' | '%'))) => *c,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.parse_power()?;
            let bin = match op {
                '*' => BinOp::Mul,
                '/' => BinOp::Div,
                _ => BinOp::Rem,
            };
            left = Expr::Binary(bin, Box::new(left), Box::new(right));
        }
    }

    // power := factor ('^' power)?    —— 右结合
    fn parse_power(&mut self) -> Result<Expr, FormulaError> {
        let base = self.parse_factor()?;
        if matches!(self.peek(), Some(Token::Op('^'))) {
            self.pos += 1;
            let exp = self.parse_power()?;
            return Ok(Expr::Binary(BinOp::Pow, Box::new(base), Box::new(exp)));
        }
        Ok(base)
    }

    // factor := ('-' | '+') factor | primary
    fn parse_factor(&mut self) -> Result<Expr, FormulaError> {
        match self.peek() {
            Some(Token::Op('-')) => {
                self.pos += 1;
                let inner = self.parse_factor()?;
                Ok(Expr::Neg(Box::new(inner)))
            }
            Some(Token::Op('+')) => {
                self.pos += 1;
                self.parse_factor()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<Expr, FormulaError> {
        match self.take() {
            Some(Token::Number(v)) => Ok(Expr::Number(v)),
            Some(Token::Ident(name)) => {
                let slot = self.slot(&name);
                Ok(Expr::Var(slot))
            }
            Some(Token::Func(name)) => {
                self.expect(Token::LParen)?;
                let mut args = Vec::new();
                if !matches!(self.peek(), Some(Token::RParen)) {
                    args.push(self.parse_expr()?);
                    while matches!(self.peek(), Some(Token::Comma)) {
                        self.pos += 1;
                        args.push(self.parse_expr()?);
                    }
                }
                self.expect(Token::RParen)?;
                Ok(Expr::Call(name, args))
            }
            Some(Token::LParen) => {
                let inner = self.parse_expr()?;
                self.expect(Token::RParen)?;
                Ok(inner)
            }
            Some(t) => Err(FormulaError::Syntax(format!("意外的记号 {:?}", t))),
            None => Err(FormulaError::Syntax("公式意外结束".into())),
        }
    }
}

// ===== 求值 =====

fn eval_expr(
    expr: &Expr,
    values: &HashMap<String, f64>,
    refs: &[String],
) -> Result<f64, FormulaError> {
    match expr {
        Expr::Number(v) => Ok(*v),
        Expr::Var(slot) => {
            let name = refs
                .get(*slot)
                .ok_or_else(|| FormulaError::Eval("引用槽位越界".into()))?;
            values
                .get(name)
                .copied()
                .ok_or_else(|| FormulaError::MissingPoint(name.clone()))
        }
        Expr::Neg(inner) => Ok(-eval_expr(inner, values, refs)?),
        Expr::Binary(op, l, r) => {
            let lv = eval_expr(l, values, refs)?;
            let rv = eval_expr(r, values, refs)?;
            match op {
                BinOp::Add => Ok(lv + rv),
                BinOp::Sub => Ok(lv - rv),
                BinOp::Mul => Ok(lv * rv),
                BinOp::Div => {
                    if rv == 0.0 {
                        Err(FormulaError::Eval("除数为零".into()))
                    } else {
                        Ok(lv / rv)
                    }
                }
                BinOp::Rem => {
                    if rv == 0.0 {
                        Err(FormulaError::Eval("取模除数为零".into()))
                    } else {
                        Ok(lv % rv)
                    }
                }
                BinOp::Pow => Ok(lv.powf(rv)),
            }
        }
        Expr::Call(name, args) => eval_call(name, args, values, refs),
    }
}

fn eval_call(
    name: &str,
    args: &[Expr],
    values: &HashMap<String, f64>,
    refs: &[String],
) -> Result<f64, FormulaError> {
    // MAX / MIN / AVG 为变参，其余为固定元数
    let mut vals = Vec::with_capacity(args.len());
    for a in args {
        vals.push(eval_expr(a, values, refs)?);
    }
    let arity_err = |want: usize| {
        FormulaError::Eval(format!(
            "函数 {} 需要 {} 个参数，实际 {} 个",
            name,
            want,
            vals.len()
        ))
    };
    match name {
        "ABS" => {
            if vals.len() != 1 {
                return Err(arity_err(1));
            }
            Ok(vals[0].abs())
        }
        "ROUND" => {
            if vals.is_empty() || vals.len() > 2 {
                return Err(arity_err(2));
            }
            let n = vals.get(1).copied().unwrap_or(0.0);
            let factor = 10f64.powf(n);
            Ok(js_round(vals[0] * factor) / factor)
        }
        "MAX" => {
            if vals.is_empty() {
                return Err(FormulaError::Eval("函数 MAX 至少需要 1 个参数".into()));
            }
            let mut acc = vals[0];
            for v in &vals[1..] {
                acc = acc.max(*v);
            }
            Ok(acc)
        }
        "MIN" => {
            if vals.is_empty() {
                return Err(FormulaError::Eval("函数 MIN 至少需要 1 个参数".into()));
            }
            let mut acc = vals[0];
            for v in &vals[1..] {
                acc = acc.min(*v);
            }
            Ok(acc)
        }
        "SQRT" => {
            if vals.len() != 1 {
                return Err(arity_err(1));
            }
            Ok(vals[0].sqrt())
        }
        "POW" => {
            if vals.len() != 2 {
                return Err(arity_err(2));
            }
            Ok(vals[0].powf(vals[1]))
        }
        "AVG" => {
            if vals.is_empty() {
                return Err(FormulaError::Eval("函数 AVG 至少需要 1 个参数".into()));
            }
            let sum: f64 = vals.iter().sum();
            Ok(sum / vals.len() as f64)
        }
        "CEIL" => {
            if vals.len() != 1 {
                return Err(arity_err(1));
            }
            Ok(vals[0].ceil())
        }
        "FLOOR" => {
            if vals.len() != 1 {
                return Err(arity_err(1));
            }
            Ok(vals[0].floor())
        }
        "LOG" => {
            if vals.len() != 1 {
                return Err(arity_err(1));
            }
            if vals[0] <= 0.0 {
                return Err(FormulaError::Eval("LOG 的参数必须为正数".into()));
            }
            Ok(vals[0].ln())
        }
        "EXP" => {
            if vals.len() != 1 {
                return Err(arity_err(1));
            }
            Ok(vals[0].exp())
        }
        "IF" => {
            if vals.len() != 3 {
                return Err(arity_err(3));
            }
            // 条件按「非零为真」，与前端 `args[0] ? a : b` 一致（NaN 视为假）
            if vals[0] != 0.0 && !vals[0].is_nan() {
                Ok(vals[1])
            } else {
                Ok(vals[2])
            }
        }
        other => Err(FormulaError::Eval(format!("不支持的函数: {}", other))),
    }
}

/// 与 JavaScript `Math.round` 一致：`.5` 向上取整（朝 +∞），而非 Rust 的远离零。
fn js_round(x: f64) -> f64 {
    (x + 0.5).floor()
}

/// 按小数位舍入（半值朝 +∞，与前端一致）
pub fn round_to_decimals(v: f64, decimals: u8) -> f64 {
    let factor = 10f64.powi(decimals as i32);
    js_round(v * factor) / factor
}

#[cfg(test)]
mod tests;
