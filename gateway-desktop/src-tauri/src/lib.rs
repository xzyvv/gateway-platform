//! 工业网关调试控制台 - 桌面客户端入口（库）
//!
//! 职责：
//! 1. Tauri 窗口壳，加载 gateway-web 前端
//! 2. 网关 HTTP 代理（绕开 CORS），前端通过 invoke 调用
//! 3. 本地配置存储（网关地址列表），存于 appConfigDir/config.json
//! 4. 系统托盘（显示/退出 + 左键切换窗口可见性）
//! 5. 自动更新（tauri-plugin-updater）

use std::path::PathBuf;
use std::sync::Arc;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

pub mod diagnostic;
pub mod health;
pub mod proxy;
pub mod scan;
pub mod state;
pub mod url_safety;

use crate::state::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // P2-5: 安装 panic hook，防止启动失败/运行期 panic 静默退出（现场黑屏）。
    // 将 panic 信息写入 crash.log + stderr，便于现场工程师取回排错。
    std::panic::set_hook(Box::new({
        let default_hook = std::panic::take_hook();
        Box::new(move |info: &std::panic::PanicHookInfo<'_>| {
            let payload = info.to_string();
            let bt = std::backtrace::Backtrace::capture();
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let msg = format!("[{ts}] PANIC: {payload}\n{bt}");
            eprintln!("{msg}");
            if let Some(dir) = dirs::config_dir() {
                let crash_path = dir.join("gateway-desktop").join("crash.log");
                let _ = std::fs::create_dir_all(
                    crash_path.parent().unwrap_or(std::path::Path::new(".")),
                );
                let _ = std::fs::write(&crash_path, format!("{msg}\n"));
            }
            tracing::error!("{msg}");
            default_hook(info);
        })
    }));

    // 配置文件路径：{用户配置目录}/gateway-desktop/config.json
    // Windows: %APPDATA%\gateway-desktop\config.json
    let config_path = dirs::config_dir()
        .map(|p| p.join("gateway-desktop").join("config.json"))
        .unwrap_or_else(|| PathBuf::from("config.json"));

    let state = Arc::new(AppState::load(config_path));

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .setup(|app| {
            // 系统托盘：使用应用默认窗口图标，菜单含"显示窗口"/"退出"两项，
            // 左键单击切换窗口可见性，避免最小化到托盘后无法唤起。
            let show_item = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // 左键单击切换窗口可见性：可见时隐藏，隐藏时显示并聚焦
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }
            let _tray = tray_builder.build(app)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            state::get_config,
            state::save_config,
            state::set_active_gateway,
            proxy::proxy_request,
            state::get_config_load_error,
            scan::scan_gateway_lan,
            health::check_gateway_health,
            health::check_url_health,
            health::forward_webhook,
            diagnostic::get_diagnostic,
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // P2-5: 启动失败不静默退出，写 crash.log + stderr 供现场排错
            let msg = format!("[启动失败] {e}");
            eprintln!("{msg}");
            if let Some(dir) = dirs::config_dir() {
                let p = dir.join("gateway-desktop").join("crash.log");
                let _ = std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new(".")));
                let _ = std::fs::write(&p, format!("{msg}\n"));
            }
            std::process::exit(1);
        });
}
