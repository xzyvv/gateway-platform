//! 网关组件管理器（配置热更新）
//!
//! 集中管理可热更新的组件：总线（BusManager）、MQTT 客户端、NTP 配置。
//! `reload_config` 根据新旧配置的差异，增量应用变更，无需重启进程。
//!
//! **原子性保证**：
//! - `reload_lock` 序列化所有 reload，防止并发配置更新导致状态错乱
//! - buses 变更：先构建新 managers/collectors，成功后再原子替换旧的
//! - mqtt 变更：先创建新客户端，成功启动后再停旧客户端（失败时保留旧的）

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::OwnedMutexGuard;

use crate::config::{
    BusConfig, ConvertRuleConfig, GatewayConfig, MqttConfig, NtpConfig, PointConfig, VirtualPoint,
};
use crate::engine::error::{GatewayError, GatewayResult};
use crate::north::mqtt::MqttClient;
use crate::platform::NtpManager;
use crate::queue::DataQueue;
use crate::south::bus::BusManager;
use crate::south::bus::SerialManager;
use crate::south::protocol::Collector;
use crate::south::protocol::ProtocolRegistry;
use crate::south::scheduler::{PointsReloadPlan, Scheduler};

/// 网关组件管理器
pub struct GatewayRuntime {
    /// 总线管理器（按 port_name 索引）
    bus_managers: parking_lot::Mutex<HashMap<String, Arc<BusManager>>>,
    /// MQTT 客户端（可热替换）
    mqtt: parking_lot::Mutex<Option<Arc<MqttClient>>>,
    scheduler: Arc<Scheduler>,
    queue: Arc<DataQueue>,
    ntp: Arc<NtpManager>,
    serial_mgr: SerialManager,
    /// 协议注册表（插件化 collector 创建）
    protocol_registry: ProtocolRegistry,
    /// 配置热重载锁：序列化所有 reload，防止并发更新导致中间状态
    reload_lock: Arc<AsyncMutex<()>>,
    /// 1.1: 指标实例（替代全局 metrics::get()/init()）
    pub metrics: Arc<crate::engine::metrics::Metrics>,
    /// 1.1: 磁盘降采样状态（替代全局 DOWNSAMPLE_TRIGGERED）
    pub disk_status: Arc<crate::engine::monitor::health::DiskStatus>,
    /// C1: 协议转换模板引擎（与 pipeline 共享实例；reload 时 swap 规则集）
    convert: Arc<crate::pipeline::ConvertEngine>,
    /// 虚拟点位引擎（与 pipeline 共享实例；reload 时 swap 规则集）
    virtual_points: Arc<crate::pipeline::VirtualPointEngine>,
}

/// `GatewayRuntime::new` 的装配参数：bootstrap 装配点唯一，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub struct GatewayComponents {
    pub bus_managers: HashMap<String, Arc<BusManager>>,
    pub mqtt: Option<Arc<MqttClient>>,
    pub scheduler: Arc<Scheduler>,
    pub queue: Arc<DataQueue>,
    pub ntp: Arc<NtpManager>,
    pub serial_mgr: SerialManager,
    pub metrics: Arc<crate::engine::metrics::Metrics>,
    pub disk_status: Arc<crate::engine::monitor::health::DiskStatus>,
    /// C1: 协议转换模板引擎（与 pipeline 共享实例；reload 时 swap 规则集）
    pub convert: Arc<crate::pipeline::ConvertEngine>,
    /// 虚拟点位引擎（与 pipeline 共享实例；reload 时 swap 规则集）
    pub virtual_points: Arc<crate::pipeline::VirtualPointEngine>,
}

/// 测试专用：构建一个「不含任何总线」的 `GatewayRuntime`。
///
/// 放在 runtime 层而非调用方是为了不违反架构守护 `north_must_not_depend_on_south`
/// （见 `gateway-core/tests/arch_deps.rs`）：装配整机必须具名 `Scheduler` /
/// `SerialManager` 等南向类型，而 `north` 禁止出现 `crate::south::` 引用。
/// 装配本就是 runtime 层职责（`GatewayComponents` 即定义于此），故跨层装配收敛到此处，
/// north 侧测试只调用本函数。
#[cfg(test)]
pub fn gateway_runtime_without_buses_for_test() -> Arc<GatewayRuntime> {
    gateway_runtime_with_serial_mgr_for_test(SerialManager::new())
}

/// 测试专用：同 [`gateway_runtime_without_buses_for_test`]，但允许注入自定义
/// [`SerialManager`]（U-5/U-6 需要假打开器验证串口交接顺序与失败自愈）。
#[cfg(test)]
pub fn gateway_runtime_with_serial_mgr_for_test(serial_mgr: SerialManager) -> Arc<GatewayRuntime> {
    use crate::config::NtpConfig;
    use crate::engine::metrics::Metrics;
    use crate::engine::monitor::health::DiskStatus;
    use crate::pipeline::{
        ConvertEngine, EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline, VirtualPointEngine,
    };
    use crate::queue::{MemoryStorage, QueueStorage};

    let metrics = Arc::new(Metrics::new());
    let queue = Arc::new(
        DataQueue::with_storage(Arc::new(MemoryStorage::new()) as Arc<dyn QueueStorage>)
            .expect("build queue"),
    );
    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: ConvertEngine::empty(),
        virtual_points: VirtualPointEngine::empty(),
        quality_rules: Arc::new(crate::pipeline::quality_rules::QualityRuleRuntime::default()),
        queue: queue.clone(),
        realtime_bus: None,
        north_tx: None,
        buffer: 16,
        metrics: metrics.clone(),
    });
    let scheduler = Arc::new(Scheduler::new(
        HashMap::new(),
        Vec::new(),
        pipeline,
        metrics.clone(),
    ));
    Arc::new(GatewayRuntime::new(GatewayComponents {
        bus_managers: HashMap::new(),
        mqtt: None,
        scheduler,
        queue,
        ntp: Arc::new(NtpManager::new(NtpConfig::default())),
        serial_mgr,
        metrics,
        disk_status: Arc::new(DiskStatus::new()),
        convert: ConvertEngine::empty(),
        virtual_points: VirtualPointEngine::empty(),
    }))
}

/// 测试专用：构建一个挂有「写结果可控」假总线（`b1`）与可写点位 `p1` 的运行时。
///
/// 供北向写回集成测试使用：写命令立即返回 `write_ok` 决定的结果，不需要真实
/// 串口/网口。与 [`gateway_runtime_without_buses_for_test`] 同理，装配收敛在
/// runtime 层，避免 `north` 侧测试出现 `crate::south::` 引用（架构守护禁止）。
#[cfg(test)]
pub fn gateway_runtime_with_fake_bus_for_test(write_ok: bool) -> Arc<GatewayRuntime> {
    use crate::config::NtpConfig;
    use crate::engine::metrics::Metrics;
    use crate::engine::model::DataPoint;
    use crate::engine::monitor::health::DiskStatus;
    use crate::pipeline::{
        ConvertEngine, EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline, VirtualPointEngine,
    };
    use crate::queue::{MemoryStorage, QueueStorage};
    use crate::south::protocol::{Collector, CollectorError, CollectorResult};
    use std::future::Future;
    use std::pin::Pin;

    struct FakeCollector {
        bus: String,
        write_ok: bool,
    }

    impl Collector for FakeCollector {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn bus_name(&self) -> &str {
            &self.bus
        }

        fn read<'a>(
            &'a self,
            _point: &'a crate::config::PointConfig,
        ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
            Box::pin(async { Err(CollectorError::Unsupported("fake bus has no data".into())) })
        }

        fn write<'a>(
            &'a self,
            _point: &'a crate::config::PointConfig,
            _value: f64,
        ) -> Pin<Box<dyn Future<Output = CollectorResult<()>> + Send + 'a>> {
            let write_ok = self.write_ok;
            Box::pin(async move {
                if write_ok {
                    Ok(())
                } else {
                    Err(CollectorError::Timeout(std::time::Duration::from_millis(1)))
                }
            })
        }
    }

    let metrics = Arc::new(Metrics::new());
    let queue = Arc::new(
        DataQueue::with_storage(Arc::new(MemoryStorage::new()) as Arc<dyn QueueStorage>)
            .expect("build queue"),
    );
    let pipeline = ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: ConvertEngine::empty(),
        virtual_points: VirtualPointEngine::empty(),
        quality_rules: Arc::new(crate::pipeline::quality_rules::QualityRuleRuntime::default()),
        queue: queue.clone(),
        realtime_bus: None,
        north_tx: None,
        buffer: 16,
        metrics: metrics.clone(),
    });

    let bus = "b1".to_string();
    let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
    collectors.insert(
        bus.clone(),
        Arc::new(FakeCollector {
            bus: bus.clone(),
            write_ok,
        }),
    );
    let point = crate::config::PointConfig {
        id: "p1".to_string(),
        bus,
        writable: true,
        ..Default::default()
    };
    let scheduler = Arc::new(Scheduler::new(
        collectors,
        vec![point],
        pipeline,
        metrics.clone(),
    ));
    Arc::new(GatewayRuntime::new(GatewayComponents {
        bus_managers: HashMap::new(),
        mqtt: None,
        scheduler,
        queue,
        ntp: Arc::new(NtpManager::new(NtpConfig::default())),
        serial_mgr: SerialManager::new(),
        metrics,
        disk_status: Arc::new(DiskStatus::new()),
        convert: ConvertEngine::empty(),
        virtual_points: VirtualPointEngine::empty(),
    }))
}

/// 判断某条总线是否应被**强制重建**（自愈 `ResetSerial` 语义）。
///
/// 规则：
/// - 仅串口总线（`tcp_addr` 为空）可强制重建——TCP 总线重开对串口恢复无意义；
/// - `force_only = Some(name)` 时只重建该条总线，避免「一条总线异常 → 全部串口总线
///   一并重开」，把局部故障放大成全站串口中断；
/// - `force_only = None` 时重建全部串口总线（配置热重载路径不使用强制重建）。
fn should_force_rebuild(bus_cfg: &BusConfig, force_serial: bool, force_only: Option<&str>) -> bool {
    force_serial
        && bus_cfg.tcp_addr.is_none()
        && force_only
            .map(|t| t == bus_cfg.port_name.as_str())
            .unwrap_or(true)
}

// ===== ROI-12：配置热重载事务（prepare → persist → commit）=====

/// 事务式热重载的失败阶段。
///
/// 契约层据此区分 HTTP 语义：prepare/commit 是运行态拒绝（配置错误），
/// persist 是落盘失败（服务端内部错误）。
#[derive(Debug)]
pub enum ConfigApplyError {
    /// 阶段 1 失败：校验/预构建失败，磁盘与运行态均未改动
    Prepare(GatewayError),
    /// 阶段 2 失败：落盘失败，运行态未改动
    Persist(String),
    /// 阶段 3 失败：串口交接失败，已自愈回滚到旧运行态
    Commit(GatewayError),
}

impl std::fmt::Display for ConfigApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigApplyError::Prepare(e) => write!(f, "{}", e),
            ConfigApplyError::Persist(e) => write!(f, "save config failed: {}", e),
            ConfigApplyError::Commit(e) => write!(f, "{}", e),
        }
    }
}

/// 串口重建项：prepare 阶段只登记，commit 阶段才 close 旧口 → open 新口。
struct SerialReopen {
    cfg: BusConfig,
    old_cfg: Option<BusConfig>,
}

/// 总线重载计划（prepare 产物）。
struct BusReloadPlan {
    new_managers: HashMap<String, Arc<BusManager>>,
    new_collectors: HashMap<String, Arc<dyn Collector>>,
    serial: Vec<SerialReopen>,
    changed: HashSet<String>,
}

impl BusReloadPlan {
    /// 本次重载后可用的总线集合（含尚未 open 的串口总线），供点位 prepare 校验。
    fn available_buses(&self) -> HashSet<String> {
        let mut buses: HashSet<String> = self.new_collectors.keys().cloned().collect();
        buses.extend(self.serial.iter().map(|s| s.cfg.port_name.clone()));
        buses
    }
}

/// MQTT 准备结果：`client` 已连上 broker 但尚未 start（不参与上报）。
///
/// 未 commit 即被丢弃时（如落盘失败）通过 Drop 发出关停信号，避免预连客户端泄漏；
/// commit 成功会把 `client` 取走，Drop 不再干预。
struct PreparedMqtt {
    client: Option<Arc<MqttClient>>,
    disable: bool,
}

impl Drop for PreparedMqtt {
    fn drop(&mut self) {
        if let Some(client) = &self.client {
            client.request_stop();
        }
    }
}

/// 一次配置热重载的全部「已准备、未上岗」资源。
///
/// 生命周期：prepare 构建 → 调用方落盘 → commit 原子交换。
/// 计划在 commit 前被丢弃时：总线 task 随 `BusManager::drop` 中止，
/// 预连 MQTT 客户端随 [`PreparedMqtt`] 的 Drop 关停，运行态保持原样。
pub struct ReloadPlan {
    /// 从 prepare 到 commit 一直持有，保证两次热重载不会交叉。
    _lock: OwnedMutexGuard<()>,
    notes: Vec<String>,
    buses: Option<BusReloadPlan>,
    points: Option<PointsReloadPlan>,
    mqtt: Option<PreparedMqtt>,
    ntp: Option<NtpConfig>,
    convert: Option<Vec<ConvertRuleConfig>>,
    virtual_points: Option<Vec<VirtualPoint>>,
}

impl GatewayRuntime {
    /// 创建网关管理器
    ///
    /// 参数经 `GatewayComponents` 显式依赖注入（装配点唯一：bootstrap）。
    pub fn new(parts: GatewayComponents) -> Self {
        let GatewayComponents {
            bus_managers,
            mqtt,
            scheduler,
            queue,
            ntp,
            serial_mgr,
            metrics,
            disk_status,
            convert,
            virtual_points,
        } = parts;
        Self {
            bus_managers: parking_lot::Mutex::new(bus_managers),
            mqtt: parking_lot::Mutex::new(mqtt),
            scheduler,
            queue,
            ntp,
            serial_mgr,
            protocol_registry: ProtocolRegistry::new(),
            reload_lock: Arc::new(AsyncMutex::new(())),
            metrics,
            disk_status,
            convert,
            virtual_points,
        }
    }

    /// 热更新配置：prepare + commit（不落盘，供非写盘路径/兼容调用）。
    ///
    /// 返回应用说明列表（供 API 响应展示）。
    /// 通过 `reload_lock` 保证同一时刻只有一个 reload 在执行。
    ///
    /// 失败时返回 `Err`，且运行态保证与旧配置一致（prepare 零副作用；
    /// 串口交接失败会恢复旧口并重建受影响采集组）。
    pub async fn reload_config(
        &self,
        old: &GatewayConfig,
        new: &GatewayConfig,
        expanded_points: Vec<PointConfig>,
    ) -> GatewayResult<Vec<String>> {
        let plan = self.prepare_reload(old, new, expanded_points).await?;
        self.commit_reload(plan).await
    }

    /// ROI-12：事务式热更新——`prepare`（可失败，零副作用）→ `persist`（落盘）
    /// → `commit`（内存交换；串口交接失败会自愈回滚）。
    ///
    /// 落盘失败时计划直接被丢弃：预建的总线 task 中止、预连 MQTT 客户端关停，
    /// 磁盘与运行态都保持旧配置，不会出现「磁盘已新、运行态仍旧」的中间态。
    pub async fn apply_config_transaction(
        &self,
        old: &GatewayConfig,
        new: &GatewayConfig,
        expanded_points: Vec<PointConfig>,
        persist: Box<dyn FnOnce() -> Result<(), String> + Send + '_>,
    ) -> Result<Vec<String>, ConfigApplyError> {
        let plan = self
            .prepare_reload(old, new, expanded_points)
            .await
            .map_err(ConfigApplyError::Prepare)?;
        // 落盘失败：plan 在此 drop，运行态零改动（prepare 不产生任何副作用）。
        persist().map_err(ConfigApplyError::Persist)?;
        self.commit_reload(plan)
            .await
            .map_err(ConfigApplyError::Commit)
    }

    /// 阶段 1：构建热重载计划（**零运行态副作用**）。
    ///
    /// - TCP 总线的新 manager/collector 在此建好但不上岗；
    /// - 串口总线只登记，commit 时才 close 旧口 → open 新口（端口独占）；
    /// - MQTT 新客户端在此连上 broker（ConnAck 成功）但尚未 start；
    /// - 点位分组差异在此算好并校验 collector 齐全。
    pub async fn prepare_reload(
        &self,
        old: &GatewayConfig,
        new: &GatewayConfig,
        expanded_points: Vec<PointConfig>,
    ) -> GatewayResult<ReloadPlan> {
        // 序列化 reload：guard 一直持有到 commit 结束，防止并发更新交叉。
        let lock = self.reload_lock.clone().lock_owned().await;

        let mut notes = Vec::new();
        let buses_changed = old.buses != new.buses;
        let mqtt_changed = old.mqtt != new.mqtt;
        let ntp_changed = old.ntp != new.ntp;
        let points_changed = old.points != new.points;
        let devices_changed = old.devices != new.devices;

        // 1. 总线与点位：总线变更时先准备新 manager/collector，再按新总线集合校验点位组。
        let (buses, points) = if buses_changed {
            let bus_plan = self.prepare_buses(&old.buses, &new.buses)?;
            let available = bus_plan.available_buses();
            let points_plan = self
                .scheduler
                .prepare_points_reload(expanded_points, &bus_plan.changed, &available)
                .await
                .map_err(GatewayError::Config)?;
            notes.push(
                "buses reloaded online (serial ports reopened, collectors rebuilt)".to_string(),
            );
            (Some(bus_plan), Some(points_plan))
        } else if points_changed || devices_changed {
            let available: HashSet<String> = self.scheduler.collectors().keys().cloned().collect();
            let points_plan = self
                .scheduler
                .prepare_points_reload(expanded_points, &HashSet::new(), &available)
                .await
                .map_err(GatewayError::Config)?;
            notes.push("points reloaded online (incremental)".to_string());
            (None, Some(points_plan))
        } else {
            (None, None)
        };

        // 2. MQTT：新客户端必须已连上 broker，否则整次保存失败（不再「静默保留旧客户端」）。
        let mqtt = if mqtt_changed {
            let prepared = self.prepare_mqtt(new.mqtt.clone()).await?;
            notes.push("mqtt reloaded online".to_string());
            Some(prepared)
        } else {
            None
        };

        // 3. NTP / 转换规则 / 虚拟点位：纯规则替换，准备阶段只取快照。
        let ntp = if ntp_changed {
            notes.push("ntp config updated online".to_string());
            Some(new.ntp.clone().unwrap_or_default())
        } else {
            None
        };
        let convert = if old.convert_rules != new.convert_rules {
            notes.push("convert rules reloaded online".to_string());
            Some(new.convert_rules.clone().unwrap_or_default())
        } else {
            None
        };
        let virtual_points = if old.virtual_points != new.virtual_points {
            notes.push("virtual points reloaded online".to_string());
            Some(new.virtual_points.clone().unwrap_or_default())
        } else {
            None
        };

        Ok(ReloadPlan {
            _lock: lock,
            notes,
            buses,
            points,
            mqtt,
            ntp,
            convert,
            virtual_points,
        })
    }

    /// 阶段 3：提交热重载（调用方必须已完成落盘）。
    ///
    /// 唯一可能失败的环节是**串口重新打开**：失败时恢复旧口与旧 collector，
    /// 并按旧点位重建受影响采集组，运行态回到本次提交前。
    pub async fn commit_reload(&self, plan: ReloadPlan) -> GatewayResult<Vec<String>> {
        let ReloadPlan {
            _lock,
            notes,
            buses,
            points,
            mqtt,
            ntp,
            convert,
            virtual_points,
        } = plan;

        let mut points = points;
        if let Some(bus_plan) = buses {
            self.apply_bus_plan(bus_plan, &mut points).await?;
        }
        if let Some(points_plan) = points.take() {
            self.scheduler.commit_points_reload(points_plan).await;
        }
        if let Some(mut prepared) = mqtt {
            if prepared.disable {
                tracing::info!("mqtt disabled by config");
                let old = self.mqtt.lock().take();
                if let Some(old) = old {
                    old.stop().await;
                }
            } else if let Some(client) = prepared.client.take() {
                // 连接已在 prepare 阶段确认：此刻 start 并原子替换，旧客户端异步停。
                client.start();
                let old = self.mqtt.lock().replace(client);
                if let Some(old) = old {
                    old.stop().await;
                    tracing::info!("old mqtt client stopped after new one connected");
                }
                tracing::info!("mqtt client reloaded (new client verified connected)");
            }
        }
        if let Some(config) = ntp {
            self.ntp.update_config(config);
        }
        if let Some(rules) = convert {
            self.convert.update_rules(rules);
        }
        if let Some(rules) = virtual_points {
            self.virtual_points.update_rules(rules);
        }

        // guard 在函数结束时释放。
        drop(_lock);
        Ok(notes)
    }

    /// 构建总线重载计划：TCP 总线直接建好 manager/collector，串口只登记待 commit 交接。
    fn prepare_buses(
        &self,
        old_buses: &[BusConfig],
        new_buses: &[BusConfig],
    ) -> GatewayResult<BusReloadPlan> {
        let old_bus_map: HashMap<&str, &BusConfig> = old_buses
            .iter()
            .map(|b| (b.port_name.as_str(), b))
            .collect();
        let old_collectors = self.scheduler.collectors();
        let old_managers = self.bus_managers.lock().clone();

        let mut new_managers: HashMap<String, Arc<BusManager>> = HashMap::new();
        let mut new_collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        let mut serial: Vec<SerialReopen> = Vec::new();
        let mut changed: HashSet<String> = HashSet::new();

        for bus_cfg in new_buses {
            let name = bus_cfg.port_name.clone();
            let old_cfg = old_bus_map.get(name.as_str()).copied();
            let is_changed = old_cfg.map(|old| old != bus_cfg).unwrap_or(true);

            if !is_changed {
                if let Some(mgr) = old_managers.get(&name) {
                    new_managers.insert(name.clone(), mgr.clone());
                }
                if let Some(col) = old_collectors.get(&name) {
                    new_collectors.insert(name, col.clone());
                }
                continue;
            }

            // 协议必须已注册：否则点位组会因缺 collector 而静默消失。
            if !self.protocol_registry.supports(&bus_cfg.protocol) {
                return Err(GatewayError::Config(format!(
                    "bus {} 建链失败：{}",
                    name,
                    crate::domain::protocol_error_message(&bus_cfg.protocol)
                )));
            }

            if bus_cfg.tcp_addr.is_some() {
                // TCP 不占独占资源，prepare 阶段即可建好（未上岗不产生业务流量）。
                let bus = Arc::new(BusManager::spawn(bus_cfg, None, self.metrics.clone()));
                let collector = self
                    .protocol_registry
                    .create(&bus_cfg.protocol, &name, bus.clone(), self.metrics.clone())
                    .ok_or_else(|| {
                        GatewayError::Config(format!("bus {} 建链失败：collector 未创建", name))
                    })?;
                new_managers.insert(name.clone(), bus);
                new_collectors.insert(name.clone(), collector);
            } else {
                serial.push(SerialReopen {
                    cfg: bus_cfg.clone(),
                    old_cfg: old_cfg.cloned(),
                });
            }
            changed.insert(name);
        }

        Ok(BusReloadPlan {
            new_managers,
            new_collectors,
            serial,
            changed,
        })
    }

    /// 提交总线计划：串口先关旧口再开新口（失败恢复旧口），最后原子交换 manager/collector。
    async fn apply_bus_plan(
        &self,
        plan: BusReloadPlan,
        points_plan: &mut Option<PointsReloadPlan>,
    ) -> GatewayResult<()> {
        let BusReloadPlan {
            mut new_managers,
            mut new_collectors,
            serial,
            changed,
        } = plan;

        let mut closed: Vec<(String, BusConfig)> = Vec::new();
        let mut opened: Vec<(String, Arc<BusManager>, Arc<dyn Collector>)> = Vec::new();

        for item in serial {
            let name = item.cfg.port_name.clone();
            // 串口独占：必须等旧 task 真正退出（端口释放）后再开新口。
            // 先把 Arc 从锁里取出并释放 guard，再 await —— 避免 MutexGuard 跨 await 点。
            let old = { self.bus_managers.lock().get(&name).cloned() };
            if let Some(old) = old {
                old.close_task(std::time::Duration::from_millis(300)).await;
            }
            if let Some(old_cfg) = &item.old_cfg {
                closed.push((name.clone(), old_cfg.clone()));
            }
            match self.serial_mgr.open_bus(&item.cfg) {
                Ok(handle) => {
                    let bus = Arc::new(BusManager::spawn(
                        &item.cfg,
                        Some(handle),
                        self.metrics.clone(),
                    ));
                    let collector = self
                        .protocol_registry
                        .create(&item.cfg.protocol, &name, bus.clone(), self.metrics.clone())
                        .ok_or_else(|| {
                            GatewayError::Config(format!("bus {} 建链失败：collector 未创建", name))
                        })?;
                    opened.push((name, bus, collector));
                }
                Err(e) => {
                    tracing::error!(
                        "commit_reload: failed to open bus {} ({}), restoring previous config",
                        name,
                        e
                    );
                    // 回滚：重建入口要申请调度器 reload 信号量，先释放点位计划持有的 permit。
                    if let Some(p) = points_plan.as_mut() {
                        p.release_permit();
                    }
                    self.restore_serial_buses(&closed).await;
                    return Err(GatewayError::Config(format!(
                        "bus {} 打开失败，配置未生效（运行态已恢复原配置）：{}",
                        name, e
                    )));
                }
            }
        }

        for (name, bus, collector) in opened {
            new_managers.insert(name.clone(), bus);
            new_collectors.insert(name, collector);
        }

        // 原子交换：先换 manager 表，再换 collector 表（两者都由新集合完整覆盖）。
        let old_managers = {
            let mut guard = self.bus_managers.lock();
            std::mem::replace(&mut *guard, new_managers)
        };
        drop(old_managers);
        *self.scheduler.collectors_lock() = new_collectors;
        tracing::info!("commit_reload: {} changed buses applied", changed.len());
        Ok(())
    }

    /// 串口交接失败后的自愈：按旧配置重开总线、重建 collector，并重建受影响采集组。
    async fn restore_serial_buses(&self, closed: &[(String, BusConfig)]) {
        for (name, cfg) in closed {
            match self.serial_mgr.open_bus(cfg) {
                Ok(handle) => {
                    let bus = Arc::new(BusManager::spawn(cfg, Some(handle), self.metrics.clone()));
                    if let Some(collector) = self.protocol_registry.create(
                        &cfg.protocol,
                        name,
                        bus.clone(),
                        self.metrics.clone(),
                    ) {
                        self.scheduler
                            .collectors_lock()
                            .insert(name.clone(), collector);
                    }
                    self.bus_managers.lock().insert(name.clone(), bus);
                }
                Err(e) => tracing::error!(
                    "restore_serial_buses: failed to reopen bus {} with previous config: {}",
                    name,
                    e
                ),
            }
        }
        // 旧采集组仍持有已关闭的旧 collector，按当前点位强制重建这些总线的组。
        let force: HashSet<String> = closed.iter().map(|(name, _)| name.clone()).collect();
        if !force.is_empty() {
            let points = self.scheduler.points();
            if let Err(e) = self
                .scheduler
                .reload_points_incremental(points, &force)
                .await
            {
                tracing::error!("restore_serial_buses: rebuild groups failed: {}", e);
            }
        }
    }

    /// C1: 协议转换命中记账快照（rule_id → 最近一次转换），供 Web 契约层透出
    pub fn convert_hits(
        &self,
    ) -> std::collections::HashMap<String, crate::pipeline::convert::ConvertHit> {
        self.convert.hits_snapshot()
    }

    /// DD-15：运行期未生效（编译失败被跳过）的转换规则（rule_id → 原因）。
    /// 规则仍保留在配置与界面中，仅标注「不生效」，与虚拟点位的错误口径一致。
    pub fn convert_issues(&self) -> std::collections::HashMap<String, String> {
        self.convert.compile_issues()
    }

    /// 虚拟点位最近求值状态快照（point_id → 状态），供 Web 契约层透出
    pub fn virtual_point_states(
        &self,
    ) -> std::collections::HashMap<String, crate::pipeline::virtual_point::VirtualPointState> {
        self.virtual_points.states()
    }

    /// 立即重算单个虚拟点位，返回最新状态（点不存在时 `None`）
    pub fn recalc_virtual_point(
        &self,
        id: &str,
    ) -> Option<crate::pipeline::virtual_point::VirtualPointState> {
        self.virtual_points.recalc(id)
    }

    /// 强制重建串口总线（供自愈 `ResetSerial` 动作复用，不新增旁路恢复逻辑）。
    ///
    /// 与配置热重载共用 [`GatewayRuntime::reload_buses`] 的重建路径：重新打开**串口**
    /// 总线（`tcp_addr` 为空）、替换 BusManager 与 collector，并重建受影响采集组；
    /// TCP 总线不受影响。走 `reload_lock` 串行化，避免与并发配置重载冲突。
    ///
    /// `only` 为 `Some(port_name)` 时**只重建该总线**（自愈按异常总线定向恢复，
    /// 避免一条总线异常时把所有正常串口总线一并重开）；`None` 重建全部串口总线。
    ///
    /// 返回实际重建的串口总线数量。`buses`/`points` 由调用方传入当前生效配置，
    /// 使 `GatewayRuntime` 无需反向依赖 `AppState`。
    /// 采集组重建失败时返回 `Err`（manager/collector 已部分替换，需由调用方上报）。
    pub async fn reset_serial_buses(
        &self,
        buses: &[BusConfig],
        points: &[PointConfig],
        only: Option<&str>,
    ) -> GatewayResult<usize> {
        let _guard = self.reload_lock.lock().await;
        self.reload_buses(buses, buses, points, true, only).await
    }

    /// 重建总线：先构建新 managers/collectors，再原子替换旧的。
    ///
    /// `force_serial` 为真时，**串口**总线（无论配置是否变化）都被强制重建
    /// （自愈 `ResetSerial` 用）；为假时仅重建配置发生变化的 bus（配置热重载用）。
    /// `force_only` 进一步限定强制重建的总线名（`None` = 全部串口总线）。
    /// 构建过程中即使部分 bus 打开失败，已打开的 bus 仍会被采用；
    /// 旧 managers 在新的构建完成后才 drop，保证采集不中断。返回被重建的 bus 数量。
    /// 采集组重建失败时返回 `Err`（managers 已替换完成，数量信息不再可靠）。
    async fn reload_buses(
        &self,
        old_buses: &[BusConfig],
        new_buses: &[BusConfig],
        points: &[PointConfig],
        force_serial: bool,
        force_only: Option<&str>,
    ) -> GatewayResult<usize> {
        use std::collections::HashSet;

        let old_bus_map: HashMap<&str, &BusConfig> = old_buses
            .iter()
            .map(|b| (b.port_name.as_str(), b))
            .collect();
        let old_collectors = self.scheduler.collectors();
        let old_managers = self.bus_managers.lock().clone();

        let mut new_managers: HashMap<String, Arc<BusManager>> = HashMap::new();
        let mut all_collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
        let mut changed_buses: HashSet<String> = HashSet::new();

        for bus_cfg in new_buses {
            let name = &bus_cfg.port_name;
            // 强制重建仅作用于串口总线（tcp_addr 为空），TCP 总线不因 ResetSerial 被无谓重启；
            // force_only 进一步把强制重建限定到自愈判定的那条异常总线。
            let forced = should_force_rebuild(bus_cfg, force_serial, force_only);
            let changed = forced
                || old_bus_map
                    .get(name.as_str())
                    .map(|old| **old != *bus_cfg)
                    .unwrap_or(true);

            if changed {
                let serial_opt = if bus_cfg.tcp_addr.is_some() {
                    None
                } else {
                    match self.serial_mgr.open_bus(bus_cfg) {
                        Ok(s) => Some(s),
                        Err(e) => {
                            // r36-APP2: 打开失败时**不能**直接 `continue` 丢弃该总线——
                            // 旧实现使其既不入 new_managers 也不入 all_collectors，而
                            // 下方 `bus_managers` 被整体替换后旧 manager 会被 drop，
                            // 该总线采集随即静默停止且不再重试（配置重载却报成功）。
                            // 此处回退保留旧 manager/collector，等价于"本次重建未生效"，
                            // 并跳过 changed_buses 计数，避免误报已重建。
                            tracing::error!(
                                "reload_buses: failed to open bus {} ({}); keeping previous instance",
                                bus_cfg.port_name,
                                e
                            );
                            if let Some(mgr) = old_managers.get(name) {
                                new_managers.insert(name.clone(), mgr.clone());
                            }
                            if let Some(col) = old_collectors.get(name) {
                                all_collectors.insert(name.clone(), col.clone());
                            }
                            continue;
                        }
                    }
                };
                let bus = Arc::new(BusManager::spawn(bus_cfg, serial_opt, self.metrics.clone()));
                new_managers.insert(bus.name().to_string(), bus.clone());

                if let Some(collector) = self.protocol_registry.create(
                    &bus_cfg.protocol,
                    &bus_cfg.port_name,
                    bus,
                    self.metrics.clone(),
                ) {
                    all_collectors.insert(bus_cfg.port_name.clone(), collector);
                } else {
                    // P0-2：registry 返回 None 既可能是协议拼错，也可能是该协议已被
                    // 当前发行 SKU 裁剪（含退役观察）。给出可操作的统一错误信息，
                    // 避免现场把「裁剪」误判为「网关不支持」。
                    tracing::error!(
                        "bus {} 建链失败：{}",
                        bus_cfg.port_name,
                        crate::domain::protocol_error_message(&bus_cfg.protocol)
                    );
                }
                changed_buses.insert(name.clone());
            } else {
                if let Some(mgr) = old_managers.get(name) {
                    new_managers.insert(name.clone(), mgr.clone());
                }
                if let Some(col) = old_collectors.get(name) {
                    all_collectors.insert(name.clone(), col.clone());
                }
            }
        }

        let old_managers = {
            let mut guard = self.bus_managers.lock();
            std::mem::replace(&mut *guard, new_managers)
        };
        drop(old_managers);
        tracing::info!(
            "reload_buses: {} changed, {} unchanged, managers swapped",
            changed_buses.len(),
            new_buses.len().saturating_sub(changed_buses.len())
        );

        *self.scheduler.collectors_lock() = all_collectors;

        self.scheduler
            .reload_points_incremental(points.to_vec(), &changed_buses)
            .await
            .map_err(GatewayError::Config)?;

        Ok(changed_buses.len())
    }

    /// 准备 MQTT 客户端：创建新客户端并等待连接成功。
    ///
    /// 关键：必须等新客户端确认连上 broker（ConnAck Success）**先于落盘**完成，
    /// 连不上则整次保存失败（旧客户端与磁盘配置都保持不变）。
    /// 新客户端连上后**不 start**，由 commit 原子替换后再启动上报。
    async fn prepare_mqtt(&self, mqtt_config: MqttConfig) -> GatewayResult<PreparedMqtt> {
        if !mqtt_config.enabled {
            return Ok(PreparedMqtt {
                client: None,
                disable: true,
            });
        }

        // S1: 加载设备证书（与启动时同一路径，跨平台化避免硬编码 Linux 路径）
        let cert_dir = std::path::Path::new(crate::shared::default_cert_dir());
        let reload_cert = {
            // Bug r11-CERT8 (fix): 注入 metrics 用于 Windows ACL 加固失败计数。
            // 非 Windows 平台该指标永远为 0（仅 error 路径递增），保留无害。
            let cm = crate::crypto::cert::CertManager::new(
                cert_dir.join("gateway.pem").as_path(),
                cert_dir.join("gateway_key.pem").as_path(),
            )
            .with_metrics(self.metrics.clone());
            cm.get_or_create().ok()
        };

        // #2 修复：MqttClient::new() 返回 Ok 仅代表 event_loop 已 spawn，
        // 不代表已连上 broker —— 必须显式等待 ConnAck。
        let new_client = MqttClient::new(
            mqtt_config,
            self.queue.clone(),
            Some(crate::shared::default_outbox_db()),
            reload_cert,
            self.metrics.clone(),
        )?;
        let new_client = Arc::new(new_client);
        // 批次 B：MQTT 热重载重建客户端后必须重新注入转换引擎，
        // 否则新客户端 payload 丢失 enum 标签（value_text）。
        new_client.set_convert_engine(self.convert.clone());

        // 等待新客户端连接成功（最多 10 秒），此期间旧客户端仍在运行，上报不中断
        let connected = new_client
            .wait_connected(std::time::Duration::from_secs(10))
            .await;

        if !connected {
            new_client.stop().await;
            return Err(GatewayError::Mqtt(
                "new mqtt client failed to connect within 10s, config rejected".into(),
            ));
        }

        Ok(PreparedMqtt {
            client: Some(new_client),
            disable: false,
        })
    }

    /// 获取当前 MQTT 客户端引用（用于优雅退出时 publish_now / stop）
    pub fn mqtt(&self) -> Option<Arc<MqttClient>> {
        self.mqtt.lock().clone()
    }

    /// 按端口名获取总线管理器引用（用于 Web API 设备发现等需直接发请求的场景）
    pub fn bus_manager(&self, name: &str) -> Option<Arc<BusManager>> {
        self.bus_managers.lock().get(name).cloned()
    }

    /// 列出所有已注册总线端口名（供 Web API 展示可扫描总线）
    pub fn bus_names(&self) -> Vec<String> {
        self.bus_managers.lock().keys().cloned().collect()
    }

    /// 按点位 id 查询当前生效的点位配置（供北向 Server 判定可写性）
    pub fn point_config(&self, id: &str) -> Option<PointConfig> {
        self.scheduler.point_by_id(id)
    }

    /// 北向写回入口（ROI-35）：与 Web 写入口共用同一套统一写服务。
    ///
    /// 白名单、`write_min/write_max` 范围校验、写结果三态、`Uncertain` 读回核实、
    /// 审计与写指标全部在 [`super::write_service::execute_writes`] 内完成。此前这里
    /// 自己实现了一遍并把三态压成 `GatewayError`（DD-34），OPC UA 因此既没有范围
    /// 校验也没有读回核实。
    pub async fn write_point_checked(
        &self,
        id: &str,
        value: f64,
        ctx: &crate::engine::write::WriteContext,
        audit: &crate::engine::audit::AuditLog,
    ) -> crate::engine::write::PointWriteResult {
        use crate::engine::write::{PointWriteResult, RejectReason, WriteMode, WriteRequest};
        let points = self.scheduler.points();
        let collectors = self.scheduler.collectors();
        let writes = [WriteRequest {
            id: id.to_string(),
            value,
        }];
        let mut results = super::write_service::execute_writes(
            &points,
            &collectors,
            &self.metrics,
            audit,
            &writes,
            WriteMode::Single,
            ctx,
        )
        .await;
        results.pop().unwrap_or_else(|| {
            PointWriteResult::rejected(
                id,
                None,
                None,
                String::new(),
                value,
                RejectReason::NotFound,
                format!("point {id} not found"),
            )
        })
    }
}

#[cfg(test)]
mod r36_tests;

// ===== r45-HEAL1：ResetSerial 定向重建 =====

#[cfg(test)]
mod r45_tests;
