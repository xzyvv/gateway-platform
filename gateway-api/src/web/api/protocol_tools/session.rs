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

/// 构造带机器码的调试错误响应（契约 §5 HTTP 层约定）。
///
/// 为什么不用 `ApiError`：`ApiError` 的 `error` 字段直接复用 `message`，
/// 只能二选一——要么前端拿到 `invalid_frame`（无现场可读信息），
/// 要么拿到中文详情（前端无法分支）。这里把两者拆开返回，
/// 既满足契约的 `error` 取值，又保留现场可读的 `message`。
///
/// 状态码：帧级失败（timeout/crc/exception/bus_busy/bus_offline）仍 200，
/// 由响应体字段表达，避免前端把「设备没回」当成接口调用失败；
/// 参数/资源错误（invalid_frame/bus_not_found/missing_bus）才用 400/404。
pub fn error_response(kind: BusErrorKind, detail: impl Into<String>) -> axum::response::Response {
    use axum::http::StatusCode;

    let (status, code) = match kind {
        BusErrorKind::InvalidFrame => (StatusCode::BAD_REQUEST, 4000),
        BusErrorKind::MissingBus => (StatusCode::BAD_REQUEST, 4001),
        BusErrorKind::UnsupportedProtocol => (StatusCode::BAD_REQUEST, 4002),
        BusErrorKind::BusNotFound => (StatusCode::NOT_FOUND, 4004),
        // 租约失败（busy/offline）给非零业务码：HTTP 仍为 200（契约 §5 不变），
        // 但前端响应拦截器只在 code!=0 时透出 message——若为 0，data=null 会把
        // 「请先暂停该总线的采集后再试」这类现场指引静默吞掉。
        BusErrorKind::BusBusy => (StatusCode::OK, 4090),
        BusErrorKind::BusOffline => (StatusCode::OK, 4503),
        _ => (StatusCode::OK, 0),
    };
    (
        status,
        axum::Json(serde_json::json!({
            "code": code,
            "message": detail.into(),
            "data": null,
            "error": kind.as_str(),
        })),
    )
        .into_response()
}
