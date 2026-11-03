//! 前后端 API 契约测试
//!
//! 扫描前端 `gateway-web/src/api/*.ts` 中所有 `url: '...'` 路径，
//! 与后端 `gateway-api/src/web/api/**/routes.rs` 中注册的路由对比，
//! 确保前端调用的每个接口在后端都有对应路由。
//!
//! 运行：`cargo test -p gateway-api --test api_contract`

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// 递归收集目录下匹配后缀的文件
fn collect_files(dir: &Path, suffix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !dir.exists() {
        return out;
    }
    for entry in walkdir(dir) {
        if entry.extension().and_then(|e| e.to_str()) == Some(suffix.trim_start_matches('.')) {
            out.push(entry);
        }
    }
    out
}

/// 简易递归目录遍历（不依赖 walkdir crate）
fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// 从后端 routes.rs 提取所有路由路径
///
/// 匹配 `.route("/api/foo/{id}", ...)` 中的路径字符串。
fn extract_backend_routes(content: &str) -> Vec<String> {
    let mut routes = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // 匹配 .route(" 或 .route(r" 开头的路径
        if let Some(idx) = trimmed.find(".route(\"") {
            let rest = &trimmed[idx + 7..];
            if let Some(end) = rest.find('"') {
                routes.push(rest[..end].to_string());
            }
        }
        if let Some(idx) = trimmed.find(".route(r#\"") {
            let rest = &trimmed[idx + 9..];
            if let Some(end) = rest.find("\"#)") {
                routes.push(rest[..end].to_string());
            }
        }
    }
    routes
}

/// 从前端 api/*.ts 提取所有 url 路径
///
/// 匹配 `url: '/api/foo'` 或 `url: "/api/foo"` 中的路径。
fn extract_frontend_urls(content: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        // 单引号
        if let Some(idx) = trimmed.find("url: '") {
            let rest = &trimmed[idx + 5..];
            if let Some(end) = rest.find('\'') {
                urls.push(rest[..end].to_string());
            }
        }
        // 双引号
        if let Some(idx) = trimmed.find("url: \"") {
            let rest = &trimmed[idx + 5..];
            if let Some(end) = rest.find('"') {
                urls.push(rest[..end].to_string());
            }
        }
    }
    urls
}

/// 归一化路径：
/// - 去掉尾部斜杠
/// - 将 `{id}` / `:id` 等参数段统一替换为 `:p`
fn normalize(path: &str) -> String {
    let mut p = path.trim_end_matches('/').to_string();
    if p.is_empty() {
        p = "/".to_string();
    }
    // 替换 axum 风格 {param} 和前端 :param
    let re_brace = regex_lite(p.as_str());
    p = re_brace;
    p
}

/// 用简单循环替换 `{...}` 和 `:word` 参数段为 `:p`
fn regex_lite(s: &str) -> String {
    let segs: Vec<String> = s
        .split('/')
        .map(|seg| {
            if (seg.starts_with('{') && seg.ends_with('}')) || seg.starts_with(':') {
                ":p".to_string()
            } else {
                seg.to_string()
            }
        })
        .collect();
    segs.join("/")
}

/// 已知的非 /api 前缀路径（健康检查、静态资源等），跳过校验
fn is_known_non_api(url: &str) -> bool {
    url.starts_with("/health")
        || url.starts_with("/ws")
        || url.starts_with("/static")
        || url.starts_with("http")
        || url.is_empty()
}

#[test]
fn frontend_urls_match_backend_routes() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();

    // 1. 收集后端路由
    let backend_dir = workspace.join("gateway-api/src/web/api");
    let backend_files = collect_files(&backend_dir, "rs");
    let mut backend_routes: HashSet<String> = HashSet::new();
    for f in &backend_files {
        if f.file_name().and_then(|n| n.to_str()) != Some("routes.rs") {
            continue;
        }
        let content = std::fs::read_to_string(f).unwrap_or_default();
        for r in extract_backend_routes(&content) {
            backend_routes.insert(normalize(&r));
        }
    }

    // 2. 收集前端 URL
    let frontend_dir = workspace.join("gateway-web/src/api");
    let frontend_files = collect_files(&frontend_dir, "ts");
    let mut frontend_urls: HashSet<String> = HashSet::new();
    for f in &frontend_files {
        let content = std::fs::read_to_string(f).unwrap_or_default();
        for u in extract_frontend_urls(&content) {
            frontend_urls.insert(u);
        }
    }

    // 3. 校验
    let mut missing: Vec<String> = Vec::new();
    for url in &frontend_urls {
        if is_known_non_api(url) {
            continue;
        }
        let norm = normalize(url);
        if !backend_routes.contains(&norm) {
            missing.push(url.clone());
        }
    }

    missing.sort();
    assert!(
        missing.is_empty(),
        "前端调用的以下 URL 在后端路由中找不到匹配（共 {} 个）:\n  {}\n后端已注册路由（{} 个）:\n  {}",
        missing.len(),
        missing.join("\n  "),
        backend_routes.len(),
        {
            let mut v: Vec<&String> = backend_routes.iter().collect();
            v.sort();
            v.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n  ")
        }
    );
}
