//! AppState mock 工厂（M2-15）
//!
//! 为鉴权集成测试构造可用的 AppState：4 个关键字段（scheduler/queue/gateway/ota）
//! 注入空实现 mock，其余具体字段用真实构造器（dummy 路径 / SQLite 临时库）。
//! 鉴权中间件只读 `state.config.web` 与 `state.web`，所以 mock 方法不会被实际调用，
//! 但 trait bound 必须满足，故提供空实现。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use gateway_api::web::traits::VirtualPointStateView;
use gateway_api::web::{
    AppState, DataQueueTrait, GatewayTrait, OtaStateView, OtaStatusView, OtaTrait, SchedulerTrait,
};
use gateway_core::config::PointConfig;
use gateway_core::config::{
    BusConfig, ConfigLoader, ConfigVersionStore, GatewayConfig, NtpConfig, TemplateManager,
    UplinkTestHistoryStore, WebConfig,
};
use gateway_core::crypto::cert::CertManager;
use gateway_core::engine::audit::AuditLog;
use gateway_core::engine::error::{GatewayError, GatewayResult};
use gateway_core::engine::metrics::Metrics;
use gateway_core::engine::model::DataPoint;
use gateway_core::engine::monitor::health::DiskHistoryStore;
use gateway_core::engine::Diagnostics;
use gateway_core::license::LicenseManager;
use gateway_core::north::mqtt::MqttClient;
use gateway_core::north::sim_consumer::SimConsumer;
use gateway_core::pipeline::RealtimeBus;
use gateway_core::platform::{GpioController, NtpManager};
use gateway_core::south::protocol::{Collector, CollectorError, CollectorResult};
use gateway_core::south::scheduler::DeviceStatus;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 每次调用 mock_app_state 自增，保证并行测试用不同的 SQLite / log 路径
static MOCK_INSTANCE: AtomicU64 = AtomicU64::new(0);

// ===== Mock 实现 =====

pub struct MockScheduler;
impl SchedulerTrait for MockScheduler {
    fn collectors(&self) -> HashMap<String, Arc<dyn Collector>> {
        HashMap::new()
    }
    fn device_status(&self) -> Vec<DeviceStatus> {
        Vec::new()
    }
}

/// 可注入设备运行态的 scheduler（自愈健康快照用）。
///
/// 自愈决策依赖 `device_status()` 里的 `bus` / `consecutive_failures` /
/// `quality`，缺了它执行器永远看不到「串口设备持续失败」这一信号。
#[allow(dead_code)]
pub struct MockSchedulerWithDevices {
    devices: Vec<DeviceStatus>,
}

#[allow(dead_code)]
impl MockSchedulerWithDevices {
    pub fn new(devices: Vec<DeviceStatus>) -> Self {
        Self { devices }
    }
}

impl SchedulerTrait for MockSchedulerWithDevices {
    fn collectors(&self) -> HashMap<String, Arc<dyn Collector>> {
        HashMap::new()
    }
    fn device_status(&self) -> Vec<DeviceStatus> {
        self.devices.clone()
    }
}

// ===== 可注入 collector 的 mock（点位读/写集成测试用）=====

/// 假 collector：read 恒返回 Good 数据点，write 结果可控（默认成功）。
/// 供点位写集成测试验证 write 成功/失败两条路径（默认 mock 的 collectors 为空表）。
#[allow(dead_code)]
pub struct MockCollector {
    bus: String,
    write_ok: bool,
}

impl MockCollector {
    #[allow(dead_code)]
    pub fn new(bus: &str) -> Self {
        Self {
            bus: bus.to_string(),
            write_ok: true,
        }
    }

    /// 设置 write 返回结果（false → 返回 Unsupported 错误，模拟写入失败）
    #[allow(dead_code)]
    pub fn with_write_result(mut self, ok: bool) -> Self {
        self.write_ok = ok;
        self
    }
}

impl Collector for MockCollector {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn bus_name(&self) -> &str {
        &self.bus
    }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
        Box::pin(async move { Ok(DataPoint::good(&point.id, "", 1.0, 1.0, "u16")) })
    }

    fn write<'a>(
        &'a self,
        _point: &'a PointConfig,
        _value: f64,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
        let ok = self.write_ok;
        Box::pin(async move {
            if ok {
                Ok(())
            } else {
                Err(CollectorError::Unsupported("mock write failure".into()))
            }
        })
    }
}

/// 携带可注入 collector 表的 scheduler（按 bus_name 索引）
#[allow(dead_code)]
pub struct MockSchedulerWithCollectors {
    collectors: HashMap<String, Arc<dyn Collector>>,
}

impl SchedulerTrait for MockSchedulerWithCollectors {
    fn collectors(&self) -> HashMap<String, Arc<dyn Collector>> {
        self.collectors.clone()
    }
    fn device_status(&self) -> Vec<DeviceStatus> {
        Vec::new()
    }
}

pub struct MockQueue;
impl DataQueueTrait for MockQueue {
    fn len<'a>(&'a self) -> Pin<Box<dyn Future<Output = usize> + Send + 'a>> {
        Box::pin(async { 0 })
    }
    fn history_ex<'a>(
        &'a self,
        _point_id: &'a str,
        _start: i64,
        _end: i64,
        _limit: usize,
        _bucket_secs: Option<u32>,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = gateway_core::engine::error::GatewayResult<
                        gateway_core::queue::HistoryQueryResult,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(gateway_core::queue::HistoryQueryResult::empty()) })
    }
}

/// 网关 mock：默认可调试总线；P0-1 相关测试可通过 [`MockGateway::set_debug_bus`]
/// 注入假总线，让 handler 走真实下发路径而不碰硬件。
pub struct MockGateway {
    debug_bus: Arc<Mutex<Option<Arc<dyn gateway_api::web::debug_bus::DebugBusTrait>>>>,
    /// `reset_serial_buses` 的调用记录（自愈定向重建验证用）。
    reset_serial_calls: Arc<Mutex<Vec<ResetSerialCall>>>,
    /// 磁盘状态实例：真实实现里由磁盘监控循环写入，这里保持同一语义，
    /// 便于测试注入「磁盘压力」信号。
    disk_status: Arc<gateway_core::engine::monitor::health::DiskStatus>,
    /// 虚拟点位求值状态（可注入，模拟 core 引擎的真实求值结果）。
    virtual_point_states: Arc<Mutex<HashMap<String, VirtualPointStateView>>>,
    /// 历次热重载的分节范围（可回读，验证「写路径确实触发了在线热重载」）。
    reload_scopes: Arc<Mutex<Vec<gateway_core::config::ReloadScope>>>,
    /// 已知存在的虚拟点位 id 集合（随配置热重载更新）。
    ///
    /// 真实实现里规则集由 core 的虚拟点位引擎承载（`ArcSwap`），
    /// 「点不存在」等价于「不在规则集里」。mock 在 `reload_config` 时按新配置
    /// 同步该集合，使 `recalc_virtual_point` 的 404 语义与真实实现一致。
    virtual_point_ids: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// 一次 `reset_serial_buses` 调用的入参快照。
///
/// `common` 目录按测试目标独立编译，各目标用到的字段不同，
/// 故整个结构体标注 `dead_code` 豁免。
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResetSerialCall {
    /// 调用方要求重建的总线名（`Some` = 定向重建，`None` = 全部串口总线）
    pub only: Option<String>,
    /// 本次传入的总线名列表
    pub buses: Vec<String>,
}

impl Default for MockGateway {
    fn default() -> Self {
        Self::new()
    }
}

impl MockGateway {
    pub fn new() -> Self {
        Self {
            debug_bus: Arc::new(Mutex::new(None)),
            reset_serial_calls: Arc::new(Mutex::new(Vec::new())),
            disk_status: Arc::new(gateway_core::engine::monitor::health::DiskStatus::new()),
            virtual_point_states: Arc::new(Mutex::new(HashMap::new())),
            reload_scopes: Arc::new(Mutex::new(Vec::new())),
            virtual_point_ids: Arc::new(Mutex::new(std::collections::HashSet::new())),
        }
    }

    /// 注入虚拟点位的求值状态（模拟 core 引擎的真实求值结果）。
    #[allow(dead_code)]
    pub fn set_virtual_point_states(&self, states: HashMap<String, VirtualPointStateView>) {
        *self.virtual_point_states.lock() = states;
    }

    /// 回读历次热重载的分节范围（验证写路径确实触发了在线热重载）。
    #[allow(dead_code)]
    pub fn reload_scopes(&self) -> Vec<gateway_core::config::ReloadScope> {
        self.reload_scopes.lock().clone()
    }

    /// 注入/替换 `debug_bus(name)` 返回的假总线
    #[allow(dead_code)]
    pub fn set_debug_bus(&self, bus: Arc<dyn gateway_api::web::debug_bus::DebugBusTrait>) {
        *self.debug_bus.lock() = Some(bus);
    }

    /// 返回 `reset_serial_buses` 的历史调用记录（自愈定向重建验证用）。
    #[allow(dead_code)]
    pub fn reset_serial_calls(&self) -> Vec<ResetSerialCall> {
        self.reset_serial_calls.lock().clone()
    }

    /// 取出本 mock 的磁盘状态实例（供测试注入磁盘压力信号）。
    #[allow(dead_code)]
    pub fn disk_status(&self) -> Arc<gateway_core::engine::monitor::health::DiskStatus> {
        self.disk_status.clone()
    }
}

impl GatewayTrait for MockGateway {
    fn reload_config<'a>(
        &'a self,
        old: &'a GatewayConfig,
        new: &'a GatewayConfig,
        _expanded_points: Vec<PointConfig>,
    ) -> Pin<
        Box<
            dyn Future<Output = gateway_core::engine::error::GatewayResult<Vec<String>>>
                + Send
                + 'a,
        >,
    > {
        // 记录本次热重载的分节范围，供测试断言「写路径确实触发了在线热重载」
        let scope = gateway_core::config::reload_scope(old, new);
        self.reload_scopes.lock().push(scope);
        // 真实实现里规则集随配置热重载（core 虚拟点位引擎 ArcSwap 替换）
        let ids: std::collections::HashSet<String> = new
            .virtual_points
            .as_ref()
            .map(|vps| vps.iter().map(|v| v.id.clone()).collect())
            .unwrap_or_default();
        *self.virtual_point_ids.lock() = ids;
        Box::pin(async { Ok(Vec::new()) })
    }
    fn reset_serial_buses<'a>(
        &'a self,
        buses: &'a [BusConfig],
        _points: &'a [PointConfig],
        only: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = gateway_core::engine::error::GatewayResult<usize>> + Send + 'a>>
    {
        self.reset_serial_calls.lock().push(ResetSerialCall {
            only: only.map(|s| s.to_string()),
            buses: buses.iter().map(|b| b.port_name.clone()).collect(),
        });
        Box::pin(async { Ok(0) })
    }
    fn mqtt(&self) -> Option<Arc<MqttClient>> {
        None
    }
    fn disk_status(&self) -> Arc<gateway_core::engine::monitor::health::DiskStatus> {
        self.disk_status.clone()
    }
    fn debug_bus(
        &self,
        _name: &str,
    ) -> Option<Arc<dyn gateway_api::web::debug_bus::DebugBusTrait>> {
        self.debug_bus.lock().clone()
    }
    fn convert_hits(
        &self,
    ) -> std::collections::HashMap<String, gateway_api::web::traits::ConvertHitView> {
        std::collections::HashMap::new()
    }
    fn virtual_point_states(
        &self,
    ) -> std::collections::HashMap<String, gateway_api::web::traits::VirtualPointStateView> {
        self.virtual_point_states.lock().clone()
    }
    fn recalc_virtual_point(
        &self,
        id: &str,
    ) -> Option<gateway_api::web::traits::VirtualPointStateView> {
        // 规则集里没有该点 → None（对应真实实现的 `rules.iter().find(...)` 失败）
        if !self.virtual_point_ids.lock().contains(id) {
            return None;
        }
        // 规则集里有该点：返回注入的状态；未注入时按「尚未求值」如实返回
        Some(self.virtual_point_states.lock().get(id).cloned().unwrap_or(
            gateway_api::web::traits::VirtualPointStateView {
                value: None,
                error: Some("尚未求值（等待采集数据）".into()),
                updated_at_ms: 0,
            },
        ))
    }
}

pub struct MockOta;
impl OtaTrait for MockOta {
    fn status(&self) -> OtaStatusView {
        OtaStatusView {
            state: OtaStateView::Idle,
            progress: 0,
            error: None,
        }
    }
    fn process_firmware(
        self: Arc<Self>,
        _data: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send>> {
        Box::pin(async {
            Err(GatewayError::Message(
                "mock: process_firmware not implemented".into(),
            ))
        })
    }
    fn process_delta(
        self: Arc<Self>,
        _data: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = GatewayResult<()>> + Send>> {
        Box::pin(async {
            Err(GatewayError::Message(
                "mock: process_delta not implemented".into(),
            ))
        })
    }
    fn rollback(&self) -> GatewayResult<()> {
        Ok(())
    }
}

// ===== mock_app_state 工厂 =====

/// 构造内存队列（供模拟消费者实例使用）。
///
/// `DataQueue` 的 actor task 由 `tokio::spawn` 拉起，需要运行时上下文。
/// mock 态统一在 `#[tokio::test]`（异步上下文）里构造，正常路径直接建即可；
/// 万一在同步上下文被调用，则临时进入一个 current_thread 运行时，
/// 至少不让构造过程 panic（actor 不推进，但 mock 语义本就允许）。
fn build_memory_queue() -> Arc<gateway_core::queue::DataQueue> {
    match tokio::runtime::Handle::try_current() {
        Ok(_) => Arc::new(gateway_core::queue::DataQueue::new_memory().expect("内存队列应可构造")),
        Err(_) => {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("临时运行时");
            let _guard = rt.enter();
            Arc::new(gateway_core::queue::DataQueue::new_memory().expect("内存队列应可构造"))
        }
    }
}

/// 构造一个最小可用 AppState：
/// - 4 个 trait 字段注入 mock
/// - 其余字段用真实构造器（dummy 路径 / SQLite 临时库）
/// - `web.api_token` 设为测试 token，鉴权中间件可正常工作
pub fn mock_app_state() -> Arc<AppState> {
    mock_app_state_with_cellular(None)
}

/// 同 [`mock_app_state`]，但预置若干条总线配置。
///
/// 设备创建类测试用：N1 起 create_device 会校验总线存在性，device 的 bus
/// 必须是配置里真实存在的 port_name。
#[allow(dead_code)]
pub fn mock_app_state_with_buses(buses: Vec<BusConfig>) -> Arc<AppState> {
    build_mock_state(None, buses, Arc::new(MockGateway::new()), HashMap::new())
}

/// 同 [`mock_app_state`]，但允许注入一个蜂窝模块实例。
///
/// 自愈执行器的 `ResetNetwork` 动作需要 `platform.cellular`，此类测试用
/// 该变体注入一个真实（可禁用 enabled=false 以避免真实拨号）的 CellularManager。
/// （仅自愈测试目标使用；common 目录按测试目标独立编译，未使用方会误报 dead_code）
#[allow(dead_code)]
pub fn mock_app_state_with_cellular(
    cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>>,
) -> Arc<AppState> {
    build_mock_state(
        cellular,
        Vec::new(),
        Arc::new(MockGateway::new()),
        HashMap::new(),
    )
}

/// 同 [`mock_app_state`]，但额外返回 mock gateway 实例。
///
/// 需要向 gateway 侧注入可变状态（如磁盘压力信号）的测试用该变体。
#[allow(dead_code)]
pub fn mock_app_state_with_gateway() -> (Arc<AppState>, Arc<MockGateway>) {
    let gateway = Arc::new(MockGateway::new());
    let state = build_mock_state(None, Vec::new(), gateway.clone(), HashMap::new());
    (state, gateway)
}

/// 构造带真实调试总线（假实现）的 AppState（P0-1 真实下发/回放集成测试用）。
///
/// `buses` 进 `GatewayConfig.buses`，handler 据 `port_name` 查得到总线配置；
/// `debug_bus` 作为 `GatewayTrait::debug_bus` 的返回值，接住 handler 下发的帧。
#[allow(dead_code)]
pub fn mock_app_state_with_debug_bus(
    buses: Vec<BusConfig>,
    debug_bus: Arc<dyn gateway_api::web::debug_bus::DebugBusTrait>,
) -> Arc<AppState> {
    let gateway = Arc::new(MockGateway::new());
    gateway.set_debug_bus(debug_bus);
    build_mock_state(None, buses, gateway, HashMap::new())
}

/// 同 [`mock_app_state_with_debug_bus`]，但额外注入已在跑采集的 collector。
///
/// 用于验证「采集引擎占用总线时调试下发被拒（bus_busy）」：
/// collector 的 key 必须等于总线 `port_name`。
#[allow(dead_code)]
pub fn mock_app_state_with_debug_bus_and_collectors(
    buses: Vec<BusConfig>,
    debug_bus: Arc<dyn gateway_api::web::debug_bus::DebugBusTrait>,
    collectors: HashMap<String, Arc<dyn Collector>>,
) -> Arc<AppState> {
    let gateway = Arc::new(MockGateway::new());
    gateway.set_debug_bus(debug_bus);
    build_mock_state(None, buses, gateway, collectors)
}

/// 构造「带设备运行态 + 可记录 `reset_serial_buses` 调用」的 AppState（自愈定向重建用）。
///
/// 返回 `(AppState, Arc<MockGateway>)`：调用方可读回 `reset_serial_calls()`
/// 断言自愈只重建了目标总线（而不是全部串口总线）。
#[allow(dead_code)]
pub fn mock_app_state_for_self_heal(
    buses: Vec<BusConfig>,
    devices: Vec<DeviceStatus>,
) -> (Arc<AppState>, Arc<MockGateway>) {
    let gateway = Arc::new(MockGateway::new());
    let state = build_mock_state_with_devices(
        None,
        buses,
        gateway.clone() as Arc<dyn GatewayTrait>,
        HashMap::new(),
        devices,
    );
    (state, gateway)
}

fn build_mock_state(
    cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>>,
    buses: Vec<BusConfig>,
    gateway: Arc<dyn GatewayTrait>,
    collectors: HashMap<String, Arc<dyn Collector>>,
) -> Arc<AppState> {
    build_mock_state_with_devices(cellular, buses, gateway, collectors, Vec::new())
}

/// 同 [`build_mock_state`]，但可注入设备运行态（自愈健康快照用）。
fn build_mock_state_with_devices(
    cellular: Option<Arc<gateway_core::platform::cellular::CellularManager>>,
    buses: Vec<BusConfig>,
    gateway: Arc<dyn GatewayTrait>,
    collectors: HashMap<String, Arc<dyn Collector>>,
    devices: Vec<DeviceStatus>,
) -> Arc<AppState> {
    let instance = MOCK_INSTANCE.fetch_add(1, Ordering::SeqCst);
    // 每个实例一个独立临时目录：tempfile 生成的名字跨进程唯一。原实现用 `instance`
    // 拼接，而 `instance` 只在本测试二进制内自增，多个测试二进制并行时首个实例
    // 都叫 0，会互相踩同一个 SQLite 库。
    // 目录不交给 TempDir 托管删除：AppState 里的 SQLite 连接与日志句柄存活到进程
    // 结束，Windows 下删不掉这些文件，残留交给系统临时目录清理。
    let tmp_dir = tempfile::Builder::new()
        .prefix("gateway_mock_")
        .tempdir()
        .expect("create temp dir")
        .keep();
    let db_path = tmp_dir.join("state.sqlite");

    // 测试模板库：create_device 自 S1 起会校验模板存在性，而原本指向不存在的目录，
    // 模板库恒为空，任何「通过 API 建设备」的测试都会拿到 400。这里落地一个最小模板
    // （协议 modbus），供设备创建类测试使用。
    let tpl_dir = tmp_dir.join("templates");
    let _ = std::fs::create_dir_all(&tpl_dir);
    let _ = std::fs::write(
        tpl_dir.join("t1.json"),
        r#"{"name":"t1","vendor":"test","model":"t1","protocol":"modbus","default_interval":5,
            "points":[{"name":"p1","unit":"","data_type":"u16","default_address":1,"function_code":3,"register_count":1,"scale":1,"offset":0,"deadband":0}]}"#,
    );
    let tpl_dir_str = tpl_dir.to_str().expect("utf8 path").to_string();

    let web = WebConfig {
        api_token: "test-token-0123456789abcdef".to_string(), // 32 chars
        username: "admin".to_string(),
        password_hash: String::new(),
        listen_addr: "127.0.0.1:0".to_string(),
        allow_anon_read: true,
        allow_default_token: true,
        realtime_buffer: 1024,
        realtime_max_subscribers: 64,
        ota_signing_key: None,
        rate_limit_per_sec: None,
        cors_origins: None,
        users: Vec::new(),
        webhook_test_allow_hosts: Vec::new(),
        webhook_test_block_private: false,
        // 测试夹具开启演示会话：回放集成测试依赖预置会话 s1/s2 的已知帧序。
        // 生产默认 false，由 `packet_replay` 单测覆盖「关闭时不返回」分支。
        demo_replay_sessions: true,
    };
    // 注意：鉴权中间件读取的是 state.config.web（动态配置），不是 state.web。
    // 因此 GatewayConfig 必须也设置 web 字段，与 state.web 保持一致。
    let config_inner = GatewayConfig {
        web: Some(web.clone()),
        buses,
        ..Default::default()
    };
    let config = Arc::new(config_inner);
    let version_store = Arc::new(
        ConfigVersionStore::new(db_path.to_str().expect("utf8 path"))
            .expect("ConfigVersionStore::new"),
    );
    let uplink_test_history = Arc::new(
        UplinkTestHistoryStore::new(db_path.to_str().expect("utf8 path"))
            .expect("UplinkTestHistoryStore::new"),
    );
    let disk_history = Arc::new(
        DiskHistoryStore::new(1440, db_path.to_str().expect("utf8 path"))
            .expect("DiskHistoryStore::new"),
    );
    // 访问控制域存储（与版本库共用同一临时库文件）
    let rate_rules = Arc::new(
        gateway_core::config::RateRuleStore::new(db_path.to_str().expect("utf8 path"))
            .expect("RateRuleStore::new"),
    );
    let quality_rule_store = Arc::new(
        gateway_core::config::QualityRuleStore::new(db_path.to_str().expect("utf8 path"))
            .expect("QualityRuleStore::new"),
    );
    let api_keys = Arc::new(
        gateway_core::config::ApiKeyStore::new(db_path.to_str().expect("utf8 path"))
            .expect("ApiKeyStore::new"),
    );
    let sched_tasks = Arc::new(
        gateway_core::config::ScheduledTaskStore::new(db_path.to_str().expect("utf8 path"))
            .expect("ScheduledTaskStore::new"),
    );

    let audit_log_path = tmp_dir.join("audit.log");
    // N-14：配置落盘路径改为按实例隔离的临时文件。原实现固定 `dummy_config.json`，
    // 用户/密码类写接口会在测试进程 CWD 留下仓库垃圾文件，且并行测试互相覆盖。
    let config_path = tmp_dir.join("config.json");
    // 重启 / 热重载类接口会真正读盘（`ConfigLoader::reload`），mock 必须落一份与
    // 内存一致的配置；否则测试会去读系统临时目录里上一次运行遗留的旧文件，
    // 并行跑多个测试二进制时还会互相踩。落盘失败不影响不读盘的用例。
    if let Ok(json) = serde_json::to_vec_pretty(&*config) {
        let _ = std::fs::write(&config_path, json);
    }

    use gateway_api::web::types::{ConfigRefs, NorthRefs, PlatformRefs, SouthRefs};
    Arc::new(AppState {
        config: ConfigRefs {
            config: arc_swap::ArcSwap::new(config),
            config_loader: ConfigLoader::new(config_path.to_str().expect("utf8 path")),
            config_reload_lock: Arc::new(tokio::sync::Mutex::new(())),
            version_store,
            templates: Arc::new(TemplateManager::new(&tpl_dir_str, &tpl_dir_str)),
            uplink_test_history,
        },
        north: NorthRefs {
            ota: Arc::new(MockOta),
            cert_manager: Arc::new(CertManager::new("dummy_cert.pem", "dummy_key.pem")),
            gateway,
            north_servers: Vec::new(),
            // N-7：测试态无 connector，状态接口按「无可用连接器」如实返回。
            // 第 5 层：ConversionManager 消费北向专用队列（此处不启动 run）。
            conversion: {
                let (_north_tx, north_rx) = gateway_core::north::NorthFeed::channel(1);
                gateway_core::north::ConversionManager::new(north_rx)
            },
            // 测试态不装配处理管道，`/apin` 不输出北向队列指标。
            north_feed: None,
            // 模拟消费者：mock 态给一个**未启动**的实例（运行态由 API 控制）。
            // enabled=true 表示「配置意图为启用」，与 running=false 的实际
            // 运行态并存——状态接口须同时体现二者。
            sim_consumer: SimConsumer::new(
                build_memory_queue(),
                gateway_core::config::SimConsumerConfig {
                    enabled: true,
                    batch_size: 10,
                    interval_ms: 50,
                    log_every: 100,
                },
                Some(Arc::new(Metrics::new())),
            ),
            // 测试态不接真实 outbox 库：死信接口按「不可用」返回，验证 503 分支。
            dead_letter: Arc::new(gateway_api::web::traits::NoopDeadLetter),
        },
        south: SouthRefs {
            scheduler: if !devices.is_empty() {
                Arc::new(MockSchedulerWithDevices { devices })
            } else if collectors.is_empty() {
                Arc::new(MockScheduler)
            } else {
                Arc::new(MockSchedulerWithCollectors { collectors })
            },
            queue: Arc::new(MockQueue),
            realtime_bus: RealtimeBus::new(1024),
            passthrough: None,
        },
        platform: PlatformRefs {
            gpio: Arc::new(Mutex::new(GpioController::new())),
            ntp: Arc::new(NtpManager::new(NtpConfig::default())),
            cellular,
            diagnostics: Diagnostics::new(tmp_dir.join("diag.log").to_str().expect("utf8 path")),
            metrics: Arc::new(Metrics::new()),
            disk_history,
            heal_events: Arc::new(gateway_api::web::heal::HealEventStore::new()),
            license: Arc::new(mock_license_manager(instance)),
            audit: Arc::new(AuditLog::new(audit_log_path)),
            rate_rules,
            quality_rule_store,
            api_keys,
            sched_tasks,
        },
        web,
        rate_limiter: Arc::new(gateway_api::web::middleware::RateLimiter::default()),
    })
}

/// 构造一个空白 LicenseManager（present=false, tier=core, status=Ok）
pub fn mock_license_manager(instance: u64) -> LicenseManager {
    // 独立临时目录承载 clock 库，理由同 `build_mock_state_with_devices`。
    let dir = tempfile::Builder::new()
        .prefix(&format!("gateway_mock_license_{instance}_"))
        .tempdir()
        .expect("create temp dir")
        .keep();
    LicenseManager::new(dir.join("clock.db").as_path())
}

/// 构造一个带预设 license 状态的 AppState（替换 platform.license 而非尝试写内部）
/// （仅 license 相关测试目标使用；common 目录按单元独立编译，未使用方会误报 dead_code）
#[allow(dead_code)]
pub fn mock_app_state_with_license(s: gateway_core::license::LicenseState) -> Arc<AppState> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static INSTANCE: AtomicU64 = AtomicU64::new(0);
    let id = INSTANCE.fetch_add(1, Ordering::SeqCst);
    let mut state = mock_app_state();
    let mgr = Arc::new(mock_license_manager(id));
    mgr.replace_state_for_test(s);
    Arc::get_mut(&mut state)
        .expect("mock_app_state just created; no other Arc holders yet")
        .platform
        .license = mgr;
    state
}

/// 构造指定 LicenseManager 的 AppState（import 签发链路测试用：需测试钥对公钥自洽）
#[allow(dead_code)]
pub fn mock_app_state_with_license_manager(
    mgr: Arc<gateway_core::license::LicenseManager>,
) -> Arc<AppState> {
    let mut state = mock_app_state();
    Arc::get_mut(&mut state)
        .expect("mock_app_state just created; no other Arc holders yet")
        .platform
        .license = mgr;
    state
}

/// 构造 AppState 并把 scheduler 替换为携带指定 collector 的 mock。
///
/// 供点位写集成测试注入假 collector：默认 [`mock_app_state`] 的 collectors 为空表，
/// handler 会在定位 collector 处提前 404，无法覆盖 write 成功路径。
/// key 必须等于 `PointConfig.bus`（handler 按 bus_name 索引）。
#[allow(dead_code)]
pub fn mock_app_state_with_collectors(
    collectors: HashMap<String, Arc<dyn Collector>>,
) -> Arc<AppState> {
    let mut state = mock_app_state();
    Arc::get_mut(&mut state)
        .expect("mock_app_state just created; no other Arc holders yet")
        .south
        .scheduler = Arc::new(MockSchedulerWithCollectors { collectors });
    state
}
