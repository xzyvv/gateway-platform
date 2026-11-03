//! 模板解析器：将模板文本编译为 Segment 序列
//!
//! 语法：`${var_name}` 占位符，变量名字符集 `[a-zA-Z0-9_]`。
//! 解析失败返回 [TemplateError]，成功返回 [TemplateEngine]（持有 segments）。

use std::fmt;

/// r35-TPL1: 变量名最大长度。`[a-zA-Z0-9_]` + 64 字符足够覆盖所有合法变量
/// （timestamp_ms / device_id / avg / max / min 等最长的也 < 30 字符）。
const MAX_VAR_NAME_LEN: usize = 64;

/// r35-TPL2: 模板文本最大字节数。典型模板 < 1KB，64KB 留 64x 余量。
/// 防恶意配置（1MB 模板）触发解析 / 渲染时内存放大。
const MAX_TEMPLATE_BYTES: usize = 64 * 1024;

/// 模板解析错误
#[derive(Debug, Clone, PartialEq)]
pub enum TemplateError {
    UnclosedVariable {
        position: usize,
    },
    InvalidVariableName {
        position: usize,
        name: String,
    },
    /// r35-TPL1: 变量名超过 64 字符上限
    VariableNameTooLong {
        position: usize,
        name: String,
    },
    /// r35-TPL2: 模板文本超过 64KB 上限
    TemplateTooLarge {
        size: usize,
    },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateError::UnclosedVariable { position } => {
                write!(f, "unclosed ${{ at position {}", position)
            }
            TemplateError::InvalidVariableName { position, name } => {
                write!(
                    f,
                    "invalid variable name '{}' at position {}",
                    name, position
                )
            }
            TemplateError::VariableNameTooLong { position, name } => {
                write!(
                    f,
                    "variable name too long ({} chars, max {}) at position {}: starts with '{}...'",
                    name.len(),
                    MAX_VAR_NAME_LEN,
                    position,
                    name.chars().take(16).collect::<String>()
                )
            }
            TemplateError::TemplateTooLarge { size } => {
                write!(
                    f,
                    "template too large: {} bytes (max {})",
                    size, MAX_TEMPLATE_BYTES
                )
            }
        }
    }
}

impl std::error::Error for TemplateError {}

/// 模板段（字面量或变量引用）
///
/// 解析器产出、渲染器消费。仅在 `template` 模块内可见。
#[derive(Debug, Clone)]
pub(super) enum Segment {
    Literal(String),
    Variable(String),
}

/// 模板渲染引擎（编译后的不可变结构）
///
/// - `new()` 在本文件（parser）
/// - `render()` / `render_bytes()` 在 `renderer.rs`
#[derive(Debug, Clone)]
pub struct TemplateEngine {
    pub(super) segments: Vec<Segment>,
}

impl TemplateEngine {
    /// 编译模板
    pub fn new(template: &str) -> Result<Self, TemplateError> {
        // r35-TPL2: 模板文本长度上限——先校验再分配 segments Vec，
        // 避免恶意 1MB 模板触发大 Vec 预分配。
        if template.len() > MAX_TEMPLATE_BYTES {
            return Err(TemplateError::TemplateTooLarge {
                size: template.len(),
            });
        }
        let mut segments = Vec::new();
        let bytes = template.as_bytes();
        let mut i = 0;
        let mut literal_start = 0;

        while i < bytes.len() {
            if i + 1 < bytes.len() && bytes[i] == b'$' && bytes[i + 1] == b'{' {
                if i > literal_start {
                    segments.push(Segment::Literal(template[literal_start..i].to_string()));
                }
                let var_start = i + 2;
                let mut close = None;
                for j in var_start..bytes.len() {
                    if bytes[j] == b'}' {
                        close = Some(j);
                        break;
                    }
                    let c = bytes[j];
                    if !is_var_char(c) {
                        // Bug: `j` 是首个非法字节，可能落在多字节 UTF-8 字符的首字节上，
                        // 此时 `j + 1` 不是字符边界，按字节切片会 panic（如 `${温度}`）。
                        // `j` 本身必为字符边界，故取 `var_start..j` 安全。
                        return Err(TemplateError::InvalidVariableName {
                            position: j,
                            name: template[var_start..j].to_string(),
                        });
                    }
                }
                match close {
                    Some(j) => {
                        let name = &template[var_start..j];
                        if name.is_empty() {
                            return Err(TemplateError::InvalidVariableName {
                                position: var_start,
                                name: String::new(),
                            });
                        }
                        // r35-TPL1: 变量名长度上限——`[a-zA-Z0-9_]` 64 字符足够。
                        // 1MB 变量名会触发大 String 分配 + render 时大键查找开销。
                        if name.len() > MAX_VAR_NAME_LEN {
                            return Err(TemplateError::VariableNameTooLong {
                                position: var_start,
                                name: name.to_string(),
                            });
                        }
                        segments.push(Segment::Variable(name.to_string()));
                        i = j + 1;
                        literal_start = i;
                    }
                    None => {
                        return Err(TemplateError::UnclosedVariable { position: i });
                    }
                }
            } else {
                i += 1;
            }
        }

        if literal_start < bytes.len() {
            segments.push(Segment::Literal(template[literal_start..].to_string()));
        }

        Ok(Self { segments })
    }
}

fn is_var_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}
