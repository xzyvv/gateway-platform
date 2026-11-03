//! 配置分片：include 机制，将主配置拆分为多个文件加载并合并
//!
//! 主配置文件可包含 `"include": ["buses.json", "points/*.json"]` 字段，
//! 引用其他配置片段文件。加载时递归展开 include，将所有片段合并为单个 JSON Value，
//! 然后走正常的 schema 校验 + 反序列化流程。
//!
//! 合并策略：
//! - 数组字段（buses/points/devices/...）：拼接（后者追加到前者末尾）
//! - 对象字段（mqtt/web/ntp/...）：深合并（后者覆盖前者同名 key，递归到子对象）
//! - 标量字段：后者覆盖前者
//! - include 字段本身：处理完毕后从合并结果中移除
//!
//! 路径解析：相对于 include 所在文件的目录
//! 支持 glob 通配符（*.json）
//! 循环引用检测 + 深度限制（防止 include 链过深）

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::engine::error::{GatewayError, GatewayResult};

/// 最大 include 嵌套深度（防止过深的 include 链或循环引用）
const MAX_INCLUDE_DEPTH: usize = 16;

/// 去掉文件开头的 UTF-8 BOM。
///
/// 现场/开发环境的 config.json 常用 Windows 记事本、PowerShell `Set-Content`
/// 等工具保存，默认会写入 BOM（EF BB BF）。serde_json 不接受 BOM，会报
/// "expected value at line 1 column 1"——对现场人员毫无指引性。
/// 设备模板加载（`config::template`）已做同样处理，这里保持一致。
fn strip_bom(content: &str) -> &str {
    content.strip_prefix('\u{feff}').unwrap_or(content)
}

/// 展开 include：递归加载所有 include 引用的文件，合并为单个 JSON Value。
///
/// 入口：传入主配置文件路径，返回合并后的 JSON Value（已移除 include 字段）。
/// 如果主配置不含 include 字段，行为等同于直接解析该文件。
pub fn expand_includes(main_path: &Path) -> GatewayResult<serde_json::Value> {
    let mut visited = HashSet::new();
    expand_recursive(main_path, &mut visited, 0)
}

/// 递归展开：读取文件 → 解析 JSON → 提取 include → 递归加载子片段 → 合并
fn expand_recursive(
    path: &Path,
    visited: &mut HashSet<PathBuf>,
    depth: usize,
) -> GatewayResult<serde_json::Value> {
    if depth > MAX_INCLUDE_DEPTH {
        return Err(GatewayError::Config(format!(
            "config include depth exceeds {} (possible circular include?)",
            MAX_INCLUDE_DEPTH
        )));
    }

    // 规范化路径用于循环检测
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if visited.contains(&canonical) {
        return Err(GatewayError::Config(format!(
            "config include circular reference detected: {}",
            canonical.display()
        )));
    }
    visited.insert(canonical.clone());

    // 读取并解析文件
    let content = fs::read_to_string(path).map_err(|e| {
        GatewayError::Config(format!(
            "read config shard {} failed: {}",
            path.display(),
            e
        ))
    })?;
    let mut value: serde_json::Value = serde_json::from_str(strip_bom(&content)).map_err(|e| {
        GatewayError::Config(format!(
            "parse config shard {} at line {} column {}: {}",
            path.display(),
            e.line(),
            e.column(),
            e
        ))
    })?;

    // 提取并处理 include 列表
    if let Some(includes) = value.get("include").cloned() {
        // 移除 include 字段（不参与合并、不进入 schema 校验）
        if let Some(obj) = value.as_object_mut() {
            obj.remove("include");
        }

        let base_dir = path.parent().unwrap_or(Path::new("."));
        let include_paths = resolve_includes(&includes, base_dir, path)?;

        for inc_path in include_paths {
            tracing::debug!(
                "config: loading include shard {} (depth {})",
                inc_path.display(),
                depth
            );
            let inc_value = expand_recursive(&inc_path, visited, depth + 1)?;
            value = merge_values(value, inc_value);
        }
    }

    // Bug 160: visited 应只表示"当前递归路径"，返回前必须移除本节点，否则合法的
    // 菱形 include（A→B、A→C，B 与 C 都引用 D）在展开 C 时会把 D 误判为循环引用
    // → 启动失败。真正的环（A→B→A）仍在下沉递归前被检出，不受影响。
    visited.remove(&canonical);
    Ok(value)
}

/// 解析 include 列表，展开 glob 通配符，返回有序的文件路径列表
fn resolve_includes(
    includes: &serde_json::Value,
    base_dir: &Path,
    source_file: &Path,
) -> GatewayResult<Vec<PathBuf>> {
    let arr = includes.as_array().ok_or_else(|| {
        GatewayError::Config(format!(
            "config 'include' field must be an array in {}",
            source_file.display()
        ))
    })?;

    let mut paths = Vec::new();
    // Bug C16: include 仅允许 base_dir 内的相对路径，禁止绝对路径与 `..` 逃逸，
    // 防止通过配置分片读取任意文件（如 ../../etc/passwd）。下面的 canonicalize
    // 包含性校验作为兜底，拦截符号链接指向 base_dir 之外的情况。
    let base_canon = base_dir.canonicalize().ok();
    for item in arr {
        let pattern = item.as_str().ok_or_else(|| {
            GatewayError::Config(format!(
                "config include items must be strings in {}",
                source_file.display()
            ))
        })?;
        let pat = Path::new(pattern);
        if pat.is_absolute() || pat.has_root() {
            return Err(GatewayError::Config(format!(
                "config include '{}' must be a relative path without '..' (referenced from {})",
                pattern,
                source_file.display()
            )));
        }
        if pat
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(GatewayError::Config(format!(
                "config include '{}' must be a relative path without '..' (referenced from {})",
                pattern,
                source_file.display()
            )));
        }
        let resolved = base_dir.join(pattern);
        let matched = glob_match(&resolved)?;
        // Bug C16: glob 命中真实文件后做 canonicalize 包含性校验，防符号链接逃逸。
        if let Some(base_canon) = &base_canon {
            for p in &matched {
                if let Ok(real) = p.canonicalize() {
                    if !real.starts_with(base_canon) {
                        return Err(GatewayError::Config(format!(
                            "config include '{}' resolves outside base dir (symlink?) (referenced from {})",
                            p.display(),
                            source_file.display()
                        )));
                    }
                }
            }
        }
        if matched.is_empty() && !pattern.contains('*') {
            // 非 glob 模式但文件不存在 → 报错（避免静默遗漏配置）
            return Err(GatewayError::Config(format!(
                "config include file not found: {} (referenced from {})",
                resolved.display(),
                source_file.display()
            )));
        }
        if matched.is_empty() && pattern.contains('*') {
            // glob 模式但无匹配 → warn（允许空匹配，如分片目录暂无文件）
            tracing::warn!(
                "config include glob '{}' matched no files (referenced from {})",
                pattern,
                source_file.display()
            );
        }
        paths.extend(matched);
    }
    Ok(paths)
}

/// 简单 glob 匹配：支持文件名中的 * 通配符
///
/// 如果路径不含 `*`，直接返回该路径。
/// 如果含 `*`，拆分为目录 + 文件名模式，列出目录并匹配。
fn glob_match(pattern: &Path) -> GatewayResult<Vec<PathBuf>> {
    let pattern_str = pattern.to_string_lossy();
    if !pattern_str.contains('*') {
        if !pattern.exists() {
            return Ok(vec![]);
        }
        return Ok(vec![pattern.to_path_buf()]);
    }

    let dir = pattern.parent().unwrap_or(Path::new("."));
    let file_pattern = pattern
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            GatewayError::Config(format!("invalid glob pattern: {}", pattern.display()))
        })?;

    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(Vec::new()),
    };
    let mut matched = Vec::new();
    for entry in entries {
        let entry = entry
            .map_err(|e| GatewayError::Config(format!("glob: read dir entry failed: {}", e)))?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if simple_glob_match(file_pattern, &name_str) {
            matched.push(entry.path());
        }
    }

    matched.sort();
    Ok(matched)
}

/// 简单通配符匹配：* 匹配任意字符序列，其他字符精确匹配
///
/// 递归实现，适用于短文件名（工业网关场景文件名通常 < 100 字符）。
fn simple_glob_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    glob_helper(&p, &t)
}

fn glob_helper(pattern: &[char], text: &[char]) -> bool {
    if pattern.is_empty() {
        return text.is_empty();
    }
    if pattern[0] == '*' {
        // * 匹配零个或多个字符：先尝试跳过 *，再尝试消费一个字符
        if glob_helper(&pattern[1..], text) {
            return true;
        }
        if !text.is_empty() && glob_helper(pattern, &text[1..]) {
            return true;
        }
        return false;
    }
    // 精确字符匹配
    if !text.is_empty() && pattern[0] == text[0] {
        return glob_helper(&pattern[1..], &text[1..]);
    }
    false
}

/// 合并两个 JSON Value：base 为基础，overlay 覆盖/扩展 base
///
/// - Object + Object → 逐 key 深合并（overlay 中的 key 覆盖或新增）
/// - Array + Array → 拼接（base 在前，overlay 在后）
/// - 其他类型 → overlay 替换 base
fn merge_values(base: serde_json::Value, overlay: serde_json::Value) -> serde_json::Value {
    match (base, overlay) {
        (serde_json::Value::Object(mut base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, overlay_val) in overlay_map {
                if let Some(base_val) = base_map.remove(&key) {
                    let merged = merge_values(base_val, overlay_val);
                    base_map.insert(key, merged);
                } else {
                    base_map.insert(key, overlay_val);
                }
            }
            serde_json::Value::Object(base_map)
        }
        (serde_json::Value::Array(mut base_arr), serde_json::Value::Array(overlay_arr)) => {
            base_arr.extend(overlay_arr);
            serde_json::Value::Array(base_arr)
        }
        (_, overlay) => overlay,
    }
}

#[cfg(test)]
mod tests;
