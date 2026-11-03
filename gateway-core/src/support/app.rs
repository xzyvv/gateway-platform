//! 网关组件管理器（配置热更新）
//!
//! 集中管理可热更新的组件：总线（BusManager）、MQTT 客户端、NTP 配置。
//! `reload_config` 根据新旧配置的差异，增量应用变更，无需重启进程。
//!
//! **原子性保证**：
//! - `reload_lock` 序列化所有 reload，防止并发配置更新导致状态错乱
//! - buses 变更：先构建新 managers/collectors，成功后再原子替换旧的
//! - mqtt 变更：先创建新客户端，成功启动后再停旧客户端（失败时保留旧的）

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex as AsyncMutex;

use crate::config::{BusConfig, GatewayConfig, MqttConfig, PointConfig};
use crate::engine::error::{GatewayError, GatewayResult};
use crate::north::mqtt::MqttClient;
use crate::platform::NtpManager;
use crate::queue::DataQueue;
use crate::south::bus::BusManager;
use crate::south::bus::SerialManager;
use crate::south::protocol::Collector;
use crate::south::protocol::ProtocolRegistry;
use crate::south::scheduler::Scheduler;

/// 网关组件管理器
pub struct Gateway {
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
    reload_lock: AsyncMutex<()>,
    /// 1.1: 指标实例（替代全局 metrics::get()/init()）
    pub metrics: Arc<crate::engine::metrics::Metrics>,
    /// 1.1: 磁盘降采样状态（替代全局 DOWNSAMPLE_TRIGGERED）
    pub disk_status: Arc<crate::engine::monitor::health::DiskStatus>,
    /// C1: 协议转换模板引擎（与 pipeline 共享实例；reload 时 swap 规则集）
    convert: Arc<crate::pipeline::ConvertEngine>,
    /// 虚拟点位引擎（与 pipeline 共享实例；reload 时 swap 规则集）
    virtual_points: Arc<crate::pipeline::VirtualPointEngine>,
}

/// `Gateway::new` 的装配参数：bootstrap 装配点唯一，
/// 用结构体承载以避免超长参数列表（clippy::too_many_arguments）。
pub struct GatewayParts {
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

/// 测试专用：构建一个「不含任何总线」的 `Gateway`。
///
/// 放在 support 层而非调用方是为了不违反架构守护 `north_must_not_depend_on_south`
/// （见 `gateway-core/tests/arch_deps.rs`）：装配整机必须具名 `Scheduler` /
/// `SerialManager` 等南向类型，而 `north` 禁止出现 `crate::south::` 引用。
/// 装配本就是 support 层职责（`GatewayParts` 即定义于此），故跨层装配收敛到此处，
/// north 侧测试只调用本函数。
#[cfg(test)]
pub fn gateway_without_buses_for_test() -> Arc<Gateway> {
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
        queue: queue.clone(),
        realtime_bus: None,
        buffer: 16,
        metrics: metrics.clone(),
    });
    let scheduler = Arc::new(Scheduler::new(
        HashMap::new(),
        Vec::new(),
        pipeline,
        metrics.clone(),
    ));
    Arc::new(Gateway::new(GatewayParts {
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

impl Gateway {
    /// 创建网关管理器
    ///
    /// 参数经 `GatewayParts` 显式依赖注入（装配点唯一：bootstrap）。
    pub fn new(parts: GatewayParts) -> Self {
        let GatewayParts {
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
            reload_lock: AsyncMutex::new(()),
            metrics,
            disk_status,
            convert,
            virtual_points,
        }
    }

    /// 热更新配置：根据新旧配置差异增量应用
    ///
    /// 返回应用说明列表（供 API 响应展示）。
    /// 通过 `reload_lock` 保证同一时刻只有一个 reload 在执行。
    ///
    /// 采集组重建失败时返回 `Err` 而不是静默忽略：此时 `collectors` / points
    /// 可能已被部分替换，调用方必须知道运行态与配置已不一致。
    pub async fn reload_config(
        &self,
        old: &GatewayConfig,
        new: &GatewayConfig,
        expanded_points: Vec<PointConfig>,
    ) -> GatewayResult<Vec<String>> {
        // 序列化 reload，防止并发配置更新
        let _guard = self.reload_lock.lock().await;

        let mut notes = Vec::new();
        let buses_changed = old.buses != new.buses;
        let mqtt_changed = old.mqtt != new.mqtt;
        let ntp_changed = old.ntp != new.ntp;
        let points_changed = old.points != new.points;
        let devices_changed = old.devices != new.devices;

        // 1. 总线变更：先构建新 managers，再原子替换
        if buses_changed {
            self.reload_buses(&old.buses, &new.buses, &expanded_points, false, None)
                .await?;
            notes.push(
                "buses reloaded online (serial ports reopened, collectors rebuilt)".to_string(),
            );
        } else if points_changed || devices_changed {
            self.scheduler
                .reload_points_incremental(expanded_points, &std::collections::HashSet::new())
                .await
                .map_err(GatewayError::Config)?;
            notes.push("points reloaded online (incremental)".to_string());
        }

        // 2. MQTT 变更：先建新客户端，成功后再停旧的
        if mqtt_changed {
            match self.reload_mqtt(new.mqtt.clone()).await {
                Ok(()) => notes.push("mqtt reloaded online".to_string()),
                Err(e) => {
                    tracing::error!("mqtt reload failed, keeping old client: {}", e);
                    notes.push(format!("mqtt reload failed (kept old): {}", e));
                }
            }
        }

        // 3. NTP 变更：在线更新
        if ntp_changed {
            let ntp_cfg = new.ntp.clone().unwrap_or_default();
            self.ntp.update_config(ntp_cfg);
            notes.push("ntp config updated online".to_string());
        }

        // 4. C1: 协议转换规则变更——引擎内 ArcSwap 原子替换规则集，无需重建 pipeline；
        //    命中记账（last_value/last_converted_at_ms）跨重载保留。
        if old.convert_rules != new.convert_rules {
            self.convert
                .update_rules(new.convert_rules.clone().unwrap_or_default());
            notes.push("convert rules reloaded online".to_string());
        }

        // 5. 虚拟点位变更——引擎内 ArcSwap 原子替换规则集，公式求值立即生效；
        //    求值状态（最近值 / 错误）跨重载保留。
        if old.virtual_points != new.virtual_points {
            self.virtual_points
                .update_rules(new.virtual_points.clone().unwrap_or_default());
            notes.push("virtual points reloaded online".to_string());
        }

        Ok(notes)
    }

    /// C1: 协议转换命中记账快照（rule_id → 最近一次转换），供 Web 契约层透出
    pub fn convert_hits(
        &self,
    ) -> std::collections::HashMap<String, crate::pipeline::convert::ConvertHit> {
        self.convert.hits_snapshot()
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
    /// 与配置热重载共用 [`Gateway::reload_buses`] 的重建路径：重新打开**串口**
    /// 总线（`tcp_addr` 为空）、替换 BusManager 与 collector，并重建受影响采集组；
    /// TCP 总线不受影响。走 `reload_lock` 串行化，避免与并发配置重载冲突。
    ///
    /// `only` 为 `Some(port_name)` 时**只重建该总线**（自愈按异常总线定向恢复，
    /// 避免一条总线异常时把所有正常串口总线一并重开）；`None` 重建全部串口总线。
    ///
    /// 返回实际重建的串口总线数量。`buses`/`points` 由调用方传入当前生效配置，
    /// 使 `Gateway` 无需反向依赖 `AppState`。
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

    /// 重建 MQTT 客户端：创建新客户端并等待连接成功后，再停旧的
    ///
    /// 关键：必须等新客户端确认连上 broker（ConnAck Success）才停旧客户端，
    /// 避免新客户端连不上时上报中断。新客户端连接失败时保留旧客户端并返回错误。
    async fn reload_mqtt(
        &self,
        mqtt_config: MqttConfig,
    ) -> crate::engine::error::GatewayResult<()> {
        if !mqtt_config.enabled {
            tracing::info!("mqtt disabled by config");
            let old = self.mqtt.lock().take();
            if let Some(old) = old {
                old.stop().await;
            }
            return Ok(());
        }

        // S1: 加载设备证书（与启动时同一路径，跨平台化避免硬编码 Linux 路径）
        let cert_dir = std::path::Path::new(crate::support::default_cert_dir());
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

        // #2 修复：先创建新客户端，等待连接成功后再替换旧客户端
        // MqttClient::new() 返回 Ok 仅代表 event_loop 已 spawn，不代表已连上 broker。
        // 若新客户端连不上（地址/证书/网络错误），必须保留旧客户端，不能先停旧的。
        let new_client = MqttClient::new(
            mqtt_config,
            self.queue.clone(),
            Some(crate::support::default_outbox_db()),
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
            // 连接失败：停掉新客户端，保留旧客户端，返回错误
            new_client.stop().await;
            return Err(crate::engine::error::GatewayError::Mqtt(
                "new mqtt client failed to connect within 10s, keeping old client".into(),
            ));
        }

        // 连接确认后才启动 publish 并原子替换旧客户端
        new_client.start();
        let old = self.mqtt.lock().replace(new_client);
        if let Some(old) = old {
            old.stop().await;
            tracing::info!("old mqtt client stopped after new one connected");
        }
        tracing::info!("mqtt client reloaded (new client verified connected)");
        Ok(())
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

    /// 北向写回入口：校验 `writable` 白名单后下发到南向采集器。
    ///
    /// 与 Web 写入口（`POST /api/points/write`）共用同一白名单语义，区别只在
    /// 调用来源与审计记录（审计由调用方按 `source` 落盘）。写结果指标
    /// `point_write_total/failures` 在此统一记账，避免每个北向通道各写一份。
    pub async fn write_point(&self, id: &str, value: f64) -> GatewayResult<()> {
        let point = self
            .scheduler
            .point_by_id(id)
            .ok_or_else(|| GatewayError::NotFound(format!("point {} not found", id)))?;
        if !point.writable {
            return Err(GatewayError::Config(format!(
                "point {} 不在写入白名单中：需在点位配置中将 writable 置为 true 后方可写入",
                point.id
            )));
        }
        let collectors = self.scheduler.collectors();
        let collector = collectors.get(&point.bus).ok_or_else(|| {
            GatewayError::NotFound(format!("collector for bus {} not found", point.bus))
        })?;
        let result = collector.write(&point, value).await;
        self.metrics.record_point_write(result.is_ok());
        result.map_err(|e| GatewayError::Protocol(e.to_string()))
    }
}

#[cfg(test)]
mod r36_tests {
    use super::*;
    use crate::config::BusConfig;

    /// r36-APP2 回归：配置热重载（ResetSerial 语义）中串口打开失败时，必须保留
    /// 该总线**已有的** manager，而不是静默丢弃。
    ///
    /// 旧实现失败即 `continue`，使该总线既不进 `new_managers` 也不进
    /// `all_collectors`；随后 `bus_managers` 被整体替换并 drop 旧 manager，
    /// 该总线采集静默停止。修复后旧实例应被回退保留，且不计入"已重建"。
    #[tokio::test]
    async fn reload_buses_keeps_old_manager_when_open_fails() {
        let gw = gateway_without_buses_for_test();

        // 预置一个"旧"串口总线 manager（serial 句柄 None，仅作占位旧实例）
        let cfg = BusConfig {
            port_name: "COM_R36_NONEXISTENT".to_string(),
            protocol: "modbus".to_string(),
            tcp_addr: None,
            ..BusConfig::default()
        };
        let old = Arc::new(BusManager::spawn(&cfg, None, gw.metrics.clone()));
        gw.bus_managers
            .lock()
            .insert(cfg.port_name.clone(), old.clone());
        assert!(gw.bus_names().contains(&cfg.port_name));

        // 强制重建：打开一个不存在的串口必定失败
        let rebuilt = gw
            .reset_serial_buses(std::slice::from_ref(&cfg), &[], None)
            .await
            .expect("reload should succeed when no points need rebuilding");

        assert_eq!(rebuilt, 0, "打开失败的总线不应计入已重建数量");
        assert!(
            gw.bus_names().contains(&cfg.port_name),
            "打开失败时旧 manager 必须被保留，不能从 bus_managers 中消失"
        );
        let now = gw.bus_manager(&cfg.port_name).expect("bus still present");
        assert!(Arc::ptr_eq(&old, &now), "应复用同一个旧 manager 实例");
    }
}

// ===== r45-HEAL1：ResetSerial 定向重建 =====

#[cfg(test)]
mod r45_tests {
    use super::*;
    use crate::config::BusConfig;

    fn serial_bus(port: &str) -> BusConfig {
        BusConfig {
            port_name: port.to_string(),
            protocol: "modbus".to_string(),
            tcp_addr: None,
            ..BusConfig::default()
        }
    }

    fn tcp_bus(port: &str) -> BusConfig {
        BusConfig {
            port_name: port.to_string(),
            protocol: "modbus".to_string(),
            tcp_addr: Some("10.0.0.2:502".to_string()),
            ..BusConfig::default()
        }
    }

    /// 强制重建判定：`force_only` 命中时才重建，且只对串口总线生效。
    #[test]
    fn should_force_rebuild_only_targets_named_serial_bus() {
        let s1 = serial_bus("/dev/ttyS1");
        let s2 = serial_bus("/dev/ttyS2");
        let tcp = tcp_bus("plc-a");

        // 指定目标：只有目标串口总线被强制重建
        assert!(should_force_rebuild(&s1, true, Some("/dev/ttyS1")));
        assert!(
            !should_force_rebuild(&s2, true, Some("/dev/ttyS1")),
            "非目标串口总线不得被强制重建"
        );
        assert!(
            !should_force_rebuild(&tcp, true, Some("plc-a")),
            "TCP 总线不得被强制重建"
        );

        // 未指定目标：重建全部串口总线（不含 TCP）
        assert!(should_force_rebuild(&s1, true, None));
        assert!(should_force_rebuild(&s2, true, None));
        assert!(!should_force_rebuild(&tcp, true, None));

        // 非强制模式（配置热重载）：一律不强制重建
        assert!(!should_force_rebuild(&s1, false, Some("/dev/ttyS1")));
        assert!(!should_force_rebuild(&s1, false, None));
    }

    /// `force_only` 传一个不存在的总线名时，不得退化成「重建全部串口总线」。
    #[test]
    fn unknown_force_only_target_rebuilds_nothing() {
        let s1 = serial_bus("/dev/ttyS1");
        assert!(!should_force_rebuild(&s1, true, Some("/dev/ttyS9")));
    }

    /// 目标名与总线名必须**完全一致**才命中（不得前缀/模糊匹配）。
    #[test]
    fn force_only_target_requires_exact_name_match() {
        let s1 = serial_bus("/dev/ttyS1");
        assert!(!should_force_rebuild(&s1, true, Some("/dev/ttyS")));
        assert!(!should_force_rebuild(&s1, true, Some("/dev/ttyS10")));
        assert!(should_force_rebuild(&s1, true, Some("/dev/ttyS1")));
    }
}
