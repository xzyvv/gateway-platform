//! Tauri 构建脚本。frontendDist 路径耦合说明见下方注释。
//!
//! 构建依赖：tauri.conf.json 的 frontendDist = "../../gateway-web/dist"，
//! 即 desktop 的构建产物嵌入依赖 gateway-web/dist/ 的存在。
//!
//! 三种构建路径的行为：
//! 1. `tauri dev` / `tauri build`：tauri CLI 先执行 beforeBuildCommand
//!    （cd ../gateway-web && npm run build），frontendDist 路径自动就绪。
//! 2. `cargo build -p gateway-desktop`（绕过 tauri CLI）：beforeBuildCommand
//!    不执行，若 gateway-web/dist/ 不存在则 tauri_build::build() 报错。
//!    需先手动 `cd gateway-web && npm run build`。
//! 3. CI desktop-build job 已保证先 build gateway-web 再 cargo check desktop。
//!
//! 若项目结构变更（gateway-web 改名/移动），需同步更新 tauri.conf.json 的
//! frontendDist / beforeDevCommand / beforeBuildCommand 三处路径。

fn main() {
    // Release 构建校验：updater 配置不得包含占位符，否则拒绝构建。
    // 防止忘记替换 pubkey/endpoint 就发布，导致自动更新功能失效或被篡改。
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    if profile == "release" {
        check_updater_placeholders();
    }
    tauri_build::build()
}

/// 读取 tauri.conf.json，检查 updater 的 pubkey 和 endpoints 是否仍为占位符。
/// 发现占位符时 panic，给出明确的修复指引。
fn check_updater_placeholders() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let config_path = std::path::Path::new(&manifest_dir).join("tauri.conf.json");

    let json = match std::fs::read_to_string(&config_path) {
        Ok(s) => s,
        Err(e) => {
            println!(
                "cargo:warning=无法读取 tauri.conf.json（{}），跳过 updater 占位符校验",
                e
            );
            return;
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(e) => {
            println!(
                "cargo:warning=tauri.conf.json JSON 解析失败（{}），跳过 updater 占位符校验",
                e
            );
            return;
        }
    };

    let updater = &value["plugins"]["updater"];
    if updater.is_null() {
        return; // 未启用 updater，跳过
    }

    let pubkey = updater["pubkey"].as_str().unwrap_or("");
    let endpoints = updater["endpoints"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();

    let mut errors = Vec::new();
    if pubkey.contains("PLACEHOLDER") {
        errors.push(format!(
            "plugins.updater.pubkey 仍为占位符 \"{}\"。\n  请生成签名密钥对并将公钥填入 tauri.conf.json。\n  生成命令：tauri signer generate -w ~/.tauri/gateway-desktop.key\n  公钥会输出到终端，私钥需保存到 CI/本地环境变量 TAURI_SIGNING_PRIVATE_KEY。",
            pubkey
        ));
    }
    if endpoints.contains("your-update-server.com") {
        errors.push(format!(
            "plugins.updater.endpoints 仍为占位符 \"{}\"。\n  请替换为实际的更新服务器 URL，格式：\n  https://your-domain.com/{{target}}/{{arch}}/{{current_version}}",
            endpoints
        ));
    }

    if !errors.is_empty() {
        let msg = errors.join("\n\n");
        panic!(
            "\n\n╔══════════════════════════════════════════════════════╗\n\
             ║  Release 构建校验失败：updater 配置包含占位符        ║\n\
             ╚══════════════════════════════════════════════════════╝\n\n\
             {}\n\n\
             修复后重新运行 npm run build。",
            msg
        );
    }
}
