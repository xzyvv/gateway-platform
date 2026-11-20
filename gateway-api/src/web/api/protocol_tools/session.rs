//! 调试会话准入（P0-1）：总线占用判定 + 同总线互斥 + 句柄获取
//!
//! 稳定性优先的取舍（契约 §6.1）：
//! - 采集引擎占用该总线时**不抢占、不自动暂停**，直接返回 `bus_busy`，
//!   由前端提示用户先暂停采集。自动暂停意味着调试请求可能让生产采集漏点，
//!   这在现场是不可接受的副作用。
//! - 同一总线同一时刻只允许一个调试会话：进程内互斥表按 `port_name` 索引，
//!   会话结束（含请求取消 / handler future 被 drop）时 `OwnedMutexGuard` 自动释放，
//!   不会留下「总线被幽灵会话永久锁死」的故障。
//! - 总线句柄来自网关已注册的 BusManager；未注册说明端口没打开/已下线，
//!   报 `bus_offline` 而不是尝试自行打开端口（那样会和采集路径抢串口）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, OnceLock};

use tokio::sync::{Mutex, OwnedMutexGuard};

use gateway_core::config::BusConfig;

use crate::web::debug_bus::{BusErrorKind, DebugBusTrait, ExchangeOutcome};
use crate::web::types::AppState;
use axum::response::IntoResponse;

/// 进程级「总线调试锁」表：key = `BusConfig.port_name`
///
/// **临时态**且不可持久化（锁没有落盘语义）：仅约束运行期互斥，
/// 重启即空，口径见 `docs/api-protocol-tools.md` §3。
fn bus_locks() -> &'static StdMutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<StdMutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| StdMutex::new(HashMap::new()))
}

fn lock_for(bus: &str) -> Arc<Mutex<()>> {
    // 持锁范围仅限「查表 + 插入」，无 await，不会阻塞其它总线
    let mut map = bus_locks().lock().unwrap_or_else(|e| e.into_inner());
    map.entry(bus.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// 独占会话句柄：总线收发接口与互斥守卫绑定为**同一个不可拆分的值**。
///
/// 为什么必须绑在一起：Rust 2021 的 async 块按「实际用到的字段」做 disjoint
/// capture。若句柄与守卫是 `DebugLease` 下并列的两个字段，写成
/// `tokio::spawn(async move { lease.handle.exchange(..) })` 时只会把 `handle`
/// 移入任务，`_guard` 留在 `spawn_*` 的栈帧里、函数返回立即 drop——总线互斥在
/// 会话真正开始前就失效，第二场调试请求能直接抢到锁（历史故障 #28）。
/// 合并后 async 块捕获的是整个 `session` 字段，守卫必然随任务走；调用方再也
/// 无法只搬走句柄，类型层面堵死这个坑。
pub struct SessionBus {
    handle: Arc<dyn DebugBusTrait>,
    _guard: OwnedMutexGuard<()>,
}

impl SessionBus {
    /// 转发一次总线收发；守卫的存活期与本次会话一致
    pub async fn exchange(
        &self,
        request: &[u8],
        timeout_ms: u64,
    ) -> Result<ExchangeOutcome, BusErrorKind> {
        self.handle.exchange(request, timeout_ms).await
    }
}

/// 一次调试会话的租约：持有期间该总线被独占，`Drop` 即释放
pub struct DebugLease {
    pub bus: BusConfig,
    /// 独占句柄（含总线互斥守卫）。必须整体移动，禁止取内部字段单用
    pub session: SessionBus,
}

#[cfg(test)]
impl DebugLease {
    /// 单元测试用租约：guard 为独立互斥锁（不占进程级总线锁表）。
    /// 回放循环只用 handle；bus 字段填最小占位配置。
    pub(crate) fn for_test(handle: Arc<dyn DebugBusTrait>) -> Self {
        let guard = Arc::new(Mutex::new(()))
            .try_lock_owned()
            .expect("新建互斥锁必可立即加锁");
        let bus: BusConfig = serde_json::from_value(serde_json::json!({
            "port_name": "test-bus",
            "protocol": "modbus",
        }))
        .expect("最小 BusConfig 可反序列化");
        DebugLease {
            bus,
            session: SessionBus {
                handle,
                _guard: guard,
            },
        }
    }
}

/// 获取调试租约失败的原因（机器码 + 给现场看的中文说明）
pub struct LeaseError {
    pub kind: BusErrorKind,
    pub detail: String,
}

/// 调试工具内部的可分类错误（机器码 + 给现场看的中文说明）。
///
/// 为什么不用 `axum::response::Response` 作 Err 变体：`Response` 体积 ≥128 字节，
/// 作为 `Result` 的 Err 会在每次返回时做大块栈拷贝（clippy::result_large_err）。
/// 这里只保留「类别 + 文案」，由调用点在最外层用 `error_response` 组装响应，
/// 响应体形状与原先完全一致。
pub struct DebugError {
    pub kind: BusErrorKind,
    pub detail: String,
}

/// 申请某条总线的独占调试权。
///
/// 判定顺序有意为之：**先看配置存在性 → 再看采集占用 → 最后抢调试锁**。
/// 顺序反了会出现「总线已被采集使用却先告知已被另一调试会话占用」，误导现场排查方向。
pub async fn acquire_bus(state: &AppState, bus_name: &str) -> Result<DebugLease, LeaseError> {
    let cfg = state.config.load_full();
    let bus = cfg
        .buses
        .iter()
        .find(|b| b.port_name == bus_name)
        .cloned()
        .ok_or_else(|| LeaseError {
            kind: BusErrorKind::BusNotFound,
            detail: format!("总线 {} 不在当前配置中", bus_name),
        })?;

    // E1：被显式暂停采集的总线视为空闲，允许调试独占。
    // 否则「请先暂停该总线的采集后再试」这条指引无路可走（原先只能停整个服务）。
    if !state.south.scheduler.is_bus_paused(bus_name)
        && state.south.scheduler.collectors().contains_key(bus_name)
    {
        return Err(LeaseError {
            kind: BusErrorKind::BusBusy,
            detail: format!(
                "总线 {} 正在被采集引擎使用，请先暂停该总线的采集后再试",
                bus_name
            ),
        });
    }

    let guard = lock_for(bus_name)
        .try_lock_owned()
        .map_err(|_| LeaseError {
            kind: BusErrorKind::BusBusy,
            detail: format!("总线 {} 已被另一调试会话占用，请等待其结束后重试", bus_name),
        })?;

    let handle = state
        .north
        .gateway
        .debug_bus(bus_name)
        .ok_or_else(|| LeaseError {
            kind: BusErrorKind::BusOffline,
            detail: format!(
                "总线 {} 不可用：端口未打开或驱动未加载，请检查配置与服务状态",
                bus_name
            ),
        })?;

    Ok(DebugLease {
        bus,
        session: SessionBus {
            handle,
            _guard: guard,
        },
    })
}

/// 写一条调试下发审计（与 OTA/点位写入同级），失败不阻断业务流程。
pub fn audit_send(state: &AppState, bus: &str, detail: &str) {
    state
        .platform
        .audit
        .log("debug.frame.send", bus, detail, "api");
}

/// 帧级/租约失败的业务码与 HTTP 状态（契约 §5 HTTP 层约定）。
///
/// 两条约束在这里同时满足：
/// - **HTTP 仍为 200**：设备没回 / 帧坏 / 总线被占用都不是「接口调用失败」，
///   前端不该按 HTTP 状态弹网络错误；
/// - **业务码必须非 0**：响应信封里 `code == 0` 是唯一成功码，前端拦截器只在
///   `code != 0` 时透出 message。此前 timeout/crc/exception 落进 `_ => (200, 0)`
///   兜底，`data: null` + `code: 0` 会让「设备未应答」被当成成功静默吞掉。
///
/// 参数/资源错误（invalid_frame/bus_not_found/missing_bus）仍用 400/404。
/// 分码口径：timeout 是「对端没回」→ 3001（与 `GatewayError::NoReply` 同码）；
/// crc / exception 是「回了但帧或异常码不对」→ 3000（协议层）。
fn error_status_and_code(kind: BusErrorKind) -> (axum::http::StatusCode, i32) {
    use crate::web::error_codes::{
        BAD_REQUEST, BUS_BUSY, BUS_MISSING, BUS_OFFLINE, INVALID_PARAM, NOT_FOUND, NO_REPLY,
        PROTOCOL,
    };
    use axum::http::StatusCode;

    match kind {
        BusErrorKind::InvalidFrame => (StatusCode::BAD_REQUEST, BAD_REQUEST),
        BusErrorKind::MissingBus => (StatusCode::BAD_REQUEST, BUS_MISSING),
        BusErrorKind::UnsupportedProtocol => (StatusCode::BAD_REQUEST, INVALID_PARAM),
        BusErrorKind::BusNotFound => (StatusCode::NOT_FOUND, NOT_FOUND),
        BusErrorKind::BusBusy => (StatusCode::OK, BUS_BUSY),
        BusErrorKind::BusOffline => (StatusCode::OK, BUS_OFFLINE),
        BusErrorKind::Timeout => (StatusCode::OK, NO_REPLY),
        BusErrorKind::Crc | BusErrorKind::Exception => (StatusCode::OK, PROTOCOL),
    }
}

/// 构造带机器码的调试错误响应。
///
/// 为什么不用 `ApiError`：`ApiError` 的 `error` 字段直接复用 `message`，
/// 只能二选一——要么前端拿到 `invalid_frame`（无现场可读信息），
/// 要么拿到中文详情（前端无法分支）。这里把两者拆开返回，
/// 既满足契约的 `error` 取值，又保留现场可读的 `message`。
pub fn error_response(kind: BusErrorKind, detail: impl Into<String>) -> axum::response::Response {
    let (status, _) = error_status_and_code(kind);
    (status, axum::Json(error_body(kind, detail.into()))).into_response()
}

/// 错误响应体：`code` / `message` / `data` / `error` / `frame_error`。
fn error_body(kind: BusErrorKind, detail: String) -> serde_json::Value {
    let (_, code) = error_status_and_code(kind);
    serde_json::json!({
        "code": code,
        "message": detail,
        "data": null,
        "error": kind.as_str(),
        // 设备/总线侧失败（HTTP 恒 200）：告诉调用方「别改请求参数，去看设备」。
        // HTTP 与业务码在这类端点上刻意分离，下游仅凭 HTTP 判会误判成成功。
        "frame_error": is_device_side_failure(kind),
    })
}

/// 失败是否由设备/总线侧引起（而非请求参数非法）。
///
/// 这五个错误对应 HTTP 200 + 非零业务码：接口调用本身是通的，失败发生在
/// 设备交互环节。其余四种（invalid_frame / missing_bus / unsupported_protocol /
/// bus_not_found）是调用方改参数就能解决的，走 400/404。
fn is_device_side_failure(kind: BusErrorKind) -> bool {
    matches!(
        kind,
        BusErrorKind::Timeout
            | BusErrorKind::Crc
            | BusErrorKind::Exception
            | BusErrorKind::BusBusy
            | BusErrorKind::BusOffline
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::error_codes;

    /// 回归守卫：帧级失败的 `code` 不能是 0。
    ///
    /// `code: 0` 在响应信封里等于成功，前端拦截器会走 `data` 分支，
    /// `data: null` 把现场指引静默吞掉——这正是 timeout/crc/exception
    /// 落进 `_ => (200, 0)` 兜底时的故障形态。
    #[test]
    fn frame_level_failures_never_report_success_code() {
        for kind in [
            BusErrorKind::Timeout,
            BusErrorKind::Crc,
            BusErrorKind::Exception,
            BusErrorKind::BusBusy,
            BusErrorKind::BusOffline,
        ] {
            let (status, code) = error_status_and_code(kind);
            assert_eq!(
                status,
                axum::http::StatusCode::OK,
                "{kind:?} 应保持 HTTP 200"
            );
            assert_ne!(code, 0, "{kind:?} 的业务码不能是成功码 0");
            assert!(
                error_codes::ALL_CODES.iter().any(|(c, _)| *c == code),
                "{kind:?} 的业务码 {code} 未登记进 ALL_CODES"
            );
        }
    }

    /// 分码口径：没回与回错必须是两个码，前端才能给出不同指引。
    #[test]
    fn timeout_and_protocol_errors_use_distinct_codes() {
        let (_, timeout) = error_status_and_code(BusErrorKind::Timeout);
        let (_, crc) = error_status_and_code(BusErrorKind::Crc);
        let (_, exception) = error_status_and_code(BusErrorKind::Exception);

        assert_eq!(timeout, error_codes::NO_REPLY);
        assert_eq!(crc, error_codes::PROTOCOL);
        assert_eq!(exception, error_codes::PROTOCOL);
        assert_ne!(timeout, crc);
    }

    /// 契约固化：`frame_error` 必须与 HTTP 200 严格等价。
    ///
    /// 这类端点的 HTTP 状态表达「接口通不通」，业务码表达「设备通不通」，
    /// 两者会分离。下游若只看 HTTP 会把这五个失败当成功，`frame_error`
    /// 给出唯一可判定的布尔位；两者不等价说明分码表被改坏了。
    #[test]
    fn frame_error_flag_matches_http_200() {
        use axum::http::StatusCode;

        for kind in [
            BusErrorKind::Timeout,
            BusErrorKind::Crc,
            BusErrorKind::Exception,
            BusErrorKind::BusBusy,
            BusErrorKind::BusOffline,
        ] {
            let (status, code) = error_status_and_code(kind);
            let body = error_body(kind, "x".into());
            assert_eq!(status, StatusCode::OK, "{kind:?}");
            assert_eq!(body["frame_error"], true, "{kind:?} 应标记为设备侧失败");
            assert_eq!(body["code"], code, "响应体码必须与状态映射同源");
            assert_ne!(body["code"], 0, "{kind:?} 不能报成功码");
        }

        for kind in [
            BusErrorKind::InvalidFrame,
            BusErrorKind::MissingBus,
            BusErrorKind::UnsupportedProtocol,
            BusErrorKind::BusNotFound,
        ] {
            let (status, _) = error_status_and_code(kind);
            let body = error_body(kind, "x".into());
            assert_ne!(status, StatusCode::OK, "{kind:?} 是调用方可纠正错误");
            assert_eq!(body["frame_error"], false, "{kind:?} 不是设备侧失败");
        }
    }
}
