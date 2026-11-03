//! 模板渲染器：将编译后的 Segment 序列 + 变量上下文渲染为输出
//!
//! 提供 [TemplateEngine::render] / [TemplateEngine::render_bytes] 两个渲染入口，
//! 以及公开的 [render_str] 字符串转义辅助。

use super::parser::{Segment, TemplateEngine};
use super::registry::TemplateVars;

impl TemplateEngine {
    /// 渲染模板，变量未找到时输出空字符串
    pub fn render(&self, vars: &TemplateVars) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                Segment::Literal(s) => out.push_str(s),
                Segment::Variable(name) => {
                    if let Some(val) = vars.lookup(name) {
                        out.push_str(&val);
                    }
                }
            }
        }
        out
    }

    /// 渲染并序列化为字节
    pub fn render_bytes(&self, vars: &TemplateVars) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.estimate_size());
        for seg in &self.segments {
            match seg {
                Segment::Literal(s) => out.extend_from_slice(s.as_bytes()),
                Segment::Variable(name) => {
                    if let Some(val) = vars.lookup(name) {
                        out.extend_from_slice(val.as_bytes());
                    }
                }
            }
        }
        out
    }

    /// R3: 返回本次渲染中「查不到值」的变量名（去重，保持首次出现顺序）。
    ///
    /// 用于在渲染前/后诊断模板变量名拼写错误，或缺少聚合配置
    /// （avg/max/min/last/sum/count 在未聚合时为 None）。
    /// 语义与 [`render`] 完全一致：某变量出现在本列表 ⇒ 渲染时输出空串。
    pub fn unresolved_vars(&self, vars: &TemplateVars) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for seg in &self.segments {
            if let Segment::Variable(name) = seg {
                if vars.lookup(name).is_none() && seen.insert(name.as_str()) {
                    out.push(name.clone());
                }
            }
        }
        out
    }

    /// R3: 渲染并同时返回未解析变量列表（诊断用）。
    ///
    /// 返回值第一项与 [`render`] 完全一致；第二项为去重后的未解析变量名。
    pub fn render_checked(&self, vars: &TemplateVars) -> (String, Vec<String>) {
        let mut out = String::new();
        let mut seen = std::collections::HashSet::new();
        let mut missing = Vec::new();
        for seg in &self.segments {
            match seg {
                Segment::Literal(s) => out.push_str(s),
                Segment::Variable(name) => match vars.lookup(name) {
                    Some(val) => out.push_str(&val),
                    None => {
                        if seen.insert(name.as_str()) {
                            missing.push(name.clone());
                        }
                    }
                },
            }
        }
        (out, missing)
    }

    fn estimate_size(&self) -> usize {
        self.segments
            .iter()
            .map(|s| match s {
                Segment::Literal(s) => s.len(),
                Segment::Variable(_) => 8,
            })
            .sum()
    }
}

/// JSON 字符串字面量渲染（转义控制字符和引号）— 公开给用户做自定义字符串包装
pub fn render_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            // r36-TPL4: 转义 bidi 控制符与行分隔符。这些字符在 JSON 中合法，
            // 但会带来 Trojan Source 式视觉欺骗（U+202A~202E / U+2066~2069 双向覆写、
            // U+200E/200F 方向标记）以及 U+2028/2029 内嵌 JavaScript 断行风险，
            // 统一按 `\uXXXX` 转义，保证输出在日志/看板/前端上下文中不可被滥用。
            '\u{2028}'
            | '\u{2029}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}' => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
