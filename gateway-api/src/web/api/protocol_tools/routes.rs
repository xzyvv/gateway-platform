//! 协议工具路由组
//!
//! 路由清单：按能力分组见下方各 `Router`（协议工具 handler 见 `handlers/`）
//!
//! 鉴权策略：
//! - GET 类：allow_anon_read=false 时要求 token
//! - POST/PUT/DELETE 类：始终要求 token
//!
//! License gate（v2.0）：所有协议工具（packet-capture / step-debug / slave-sim /
//! protocol-scan / packet-editor / protocol-convert）全部免费，不挂中间件。
//! 调试能力永远不被授权状态阻断——这是 v2.0 模型的核心承诺。

use std::sync::Arc;

use axum::{
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post, put},
    Router,
};

use super::{
    clear_packets, create_convert_rule, delete_convert_rule, delete_register_scan_result,
    delete_replay_session, execute_replay, generate_register_suggestions, get_auto_detect_result,
    get_protocol_diag, get_register_scan_result, get_replay_session, get_simulator_status,
    list_convert_rules, list_packets, list_presets, list_protocols, list_register_scan_results,
    list_replay_sessions, list_sim_registers, save_replay_session, scan_slaves, send_frame,
    start_auto_detect, start_register_scan, start_simulator, stop_auto_detect, stop_simulator,
    toggle_convert_rule, update_convert_rule, update_sim_register,
};
use crate::web::types::{require_auth, require_auth_if_anon_disabled, AppState};

/// 构建协议工具路由组
pub fn protocol_tools_routes(state: Arc<AppState>) -> Router<Arc<AppState>> {
    // ===== protocol-scan + protocol-convert（v2.0 全部免费）=====
    let protocol_scan_read = Router::new()
        // P0-5：参数探测改为 start→poll，这里查询探测结果
        .route(
            "/api/protocol-tools/auto-detect/run/:id",
            get(get_auto_detect_result),
        )
        .route(
            "/api/protocol-tools/register-scan",
            get(list_register_scan_results),
        )
        .route(
            "/api/protocol-tools/register-scan/:id",
            get(get_register_scan_result),
        )
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let protocol_scan_write = Router::new()
        // P0-5：参数探测真实化（真实发帧验证当前配置下可达性），start→poll
        .route("/api/protocol-tools/auto-detect/run", post(start_auto_detect))
        .route(
            "/api/protocol-tools/auto-detect/run/:id/stop",
            post(stop_auto_detect),
        )
        .route(
            "/api/protocol-tools/register-scan",
            post(start_register_scan),
        )
        // P3-4：register-scan/:id 的 DELETE 已内含「先中止再移除」语义，
        // 单独的 /:id/stop 无调用方，删除避免同一能力两条路由
        .route(
            "/api/protocol-tools/register-scan/:id",
            delete(delete_register_scan_result),
        )
        .route(
            "/api/protocol-tools/register-scan/:id/suggestions",
            post(generate_register_suggestions),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // ===== packet-capture + packet-replay（v2.0 全部免费）=====
    // P3-4：原 /packet-capture/buses 无调用方（前端总线列表统一走 useDebugTargets
    // → GET /api/config，抓包筛选值必须与帧记录口径同源），已删除
    let packet_capture_read = Router::new()
        .route(
            "/api/protocol-tools/packet-capture/packets",
            get(list_packets),
        )
        .route(
            "/api/protocol-tools/packet-replay/sessions",
            get(list_replay_sessions),
        )
        .route(
            "/api/protocol-tools/packet-replay/sessions/:id",
            get(get_replay_session),
        )
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let packet_capture_write = Router::new()
        .route(
            "/api/protocol-tools/packet-capture/clear",
            post(clear_packets),
        )
        // 回放会话 CRUD（持久化到 sessions.json，预置会话不可删/不可覆盖）
        .route(
            "/api/protocol-tools/packet-replay/sessions",
            post(save_replay_session),
        )
        .route(
            "/api/protocol-tools/packet-replay/sessions/:id",
            delete(delete_replay_session),
        )
        // P0-1：会话真实回放（逐帧重发录制请求并对比真实响应）
        .route(
            "/api/protocol-tools/packet-replay/sessions/:id/execute",
            post(execute_replay),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // ===== packet-editor（v2.0 全部免费）=====
    let packet_editor_read = Router::new()
        .route(
            "/api/protocol-tools/packet-editor/presets",
            get(list_presets),
        )
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let packet_editor_write = Router::new()
        .route("/api/protocol-tools/packet-editor/send", post(send_frame))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // ===== protocol-convert（v2.0 全部免费）=====
    let convert_read = Router::new()
        .route("/api/protocol-tools/convert/rules", get(list_convert_rules))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let convert_write = Router::new()
        .route(
            "/api/protocol-tools/convert/rules",
            post(create_convert_rule),
        )
        .route(
            "/api/protocol-tools/convert/rules/:id",
            put(update_convert_rule).delete(delete_convert_rule),
        )
        .route(
            "/api/protocol-tools/convert/rules/:id/toggle",
            post(toggle_convert_rule),
        )
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // step-debug 后端已删除：前端单步调试（StepDebug.vue）改为每步直调
    // /api/protocol-tools/packet-editor/send 真实下发，执行日志存页面本地；
    // 原内存队列（static Mutex 全局态，多用户互相覆盖日志）无调用方。
    //
    // protocol-test（协议一致性测试）同理已删除：面向协议开发者的自测工具，
    // 12 项用例固定打 0xFFFF 非法地址，现场工程师无法处置其结论；
    // 需要验帧请用 packet-capture / packet-editor / 协议扫描。

    // ===== slave-sim（v2.0 全部免费）=====
    let slave_sim_read = Router::new()
        .route(
            "/api/protocol-tools/simulator/registers",
            get(list_sim_registers),
        )
        .route(
            "/api/protocol-tools/simulator/status",
            get(get_simulator_status),
        )
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    let slave_sim_write = Router::new()
        .route(
            "/api/protocol-tools/simulator/registers/:addr",
            put(update_sim_register),
        )
        .route("/api/protocol-tools/simulator/start", post(start_simulator))
        .route("/api/protocol-tools/simulator/stop", post(stop_simulator))
        .route("/api/protocol-tools/slave-scan", post(scan_slaves))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    // ===== 公开 tier 列表（core 即可，列已注册协议清单）=====
    // P1：协议诊断（s7comm SZL / bacnet Who-Is / gpio 芯片信息）此前无任何生产
    // 调用方，能力位 supports_diag 下发却没入口 —— 这里与协议清单同组暴露，同样免费。
    let public_read = Router::new()
        .route("/api/protocols", get(list_protocols))
        .route("/api/protocols/:name/diag", get(get_protocol_diag))
        .route_layer(from_fn_with_state(
            state.clone(),
            require_auth_if_anon_disabled,
        ));

    Router::new()
        .merge(protocol_scan_read)
        .merge(protocol_scan_write)
        .merge(packet_capture_read)
        .merge(packet_capture_write)
        .merge(packet_editor_read)
        .merge(packet_editor_write)
        .merge(convert_read)
        .merge(convert_write)
        .merge(slave_sim_read)
        .merge(slave_sim_write)
        .merge(public_read)
        .route_layer(from_fn(crate::web::middleware::tag_protocol_tool))
}
