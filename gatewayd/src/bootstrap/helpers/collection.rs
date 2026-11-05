//! 采集链辅助函数（Step 2-7：logging/watchdog/NTP/GPIO/串口/采集器）
//!
//! 对应 bootstrap 三段拆分中的 CollectionChain 阶段。

use std::collections::HashMap;
use std::sync::Arc;
// Duration 仅在 linux 平台的硬件看门狗启动分支中使用，cfg-gate 避免其他平台 unused 警告。
#[cfg(target_os = "linux")]
use std::time::Duration;

use anyhow::Result;
use parking_lot::Mutex;
use tracing::{info, warn};

use gateway_core::config::{GatewayConfig, NtpConfig, PointConfig, TemplateManager};
#[cfg(target_os = "linux")]
use gateway_core::platform::Watchdog;
use gateway_core::platform::{GpioController, NtpManager};
use gateway_core::south::bus::{BusManager, SerialManager};
use gateway_core::south::protocol::Collector;

use gateway_core::support::{default_template_builtin, default_template_custom};

use super::super::CollectionChain;

/// 打开所有串口/TCP 总线，返回 SerialManager 和按端口名索引的 BusManager 映射。
///
/// 打开失败的总线会被跳过并汇总到单条 warn 日志（Bug 86），
/// 避免 N 条 warn 被海量 trace 淹没。
pub(crate) fn open_serial_buses(
    config: &GatewayConfig,
    metrics: Arc<gateway_core::engine::metrics::Metrics>,
) -> (SerialManager, HashMap<String, Arc<BusManager>>) {
    let serial_mgr = SerialManager::new();
    let mut bus_managers: HashMap<String, Arc<BusManager>> = HashMap::new();
    let mut bus_open_errors = Vec::new();

    for bus_cfg in &config.buses {
        let serial = if bus_cfg.tcp_addr.is_some() {
            None
        } else {
            match serial_mgr.open_bus(bus_cfg) {
                Ok(s) => Some(s),
                Err(e) => {
                    bus_open_errors.push((bus_cfg.port_name.clone(), e.to_string()));
                    continue;
                }
            }
        };
        let bm = BusManager::spawn(bus_cfg, serial, metrics.clone());
        bus_managers.insert(bus_cfg.port_name.clone(), Arc::new(bm));
    }

    if !bus_open_errors.is_empty() {
        let ports: Vec<&str> = bus_open_errors.iter().map(|(p, _)| p.as_str()).collect();
        warn!(
            "failed to open {}/{} bus(es): [{}]",
            bus_open_errors.len(),
            config.buses.len(),
            ports.join(", ")
        );
        for (port, err) in &bus_open_errors {
            warn!("  bus '{}': {}", port, err);
        }
    }
    info!(
        "opened {} bus(es){}",
        bus_managers.len(),
        if bus_open_errors.is_empty() {
            ""
        } else {
            " (with errors, see above)"
        }
    );

    (serial_mgr, bus_managers)
}

/// `init_collectors` 的返回三元组：按总线名索引的 collector 表、展开后的点位列表、模板管理器。
/// 抽取为类型别名，避免函数签名触发 clippy::type_complexity。
pub(crate) type CollectorRegistry = (
    HashMap<String, Arc<dyn Collector>>,
    Vec<PointConfig>,
    Arc<TemplateManager>,
);

/// 初始化 Collector（注册表 + 按总线创建）。
///
/// 返回 (collectors 按总线名索引, 展开后的点位列表, TemplateManager)。
/// Bug 82: collector key 必须与 scheduler 查找一致——按总线名索引。
pub(crate) fn init_collectors(
    config: &GatewayConfig,
    bus_managers: &HashMap<String, Arc<BusManager>>,
    metrics: Arc<gateway_core::engine::metrics::Metrics>,
) -> Result<CollectorRegistry> {
    let mut collectors: HashMap<String, Arc<dyn Collector>> = HashMap::new();
    let protocol_registry = gateway_core::south::protocol::ProtocolRegistry::new();

    let templates = Arc::new(TemplateManager::new(
        default_template_builtin(),
        default_template_custom(),
    ));
    let expanded_points = expand_config_points(config, &templates)?;

    for point in &expanded_points {
        // GPIO 协议不需要 BusManager（直接通过 /dev/gpiochip0 读取），
        // 单独构造 GpioCollector 注册，不经过 protocol_registry.create
        if point.protocol == "gpio" {
            let key = point.bus.clone();
            if let std::collections::hash_map::Entry::Vacant(e) = collectors.entry(key) {
                let collector: Arc<dyn Collector> = Arc::new(
                    gateway_core::south::protocol::gpio::GpioCollector::new(&point.bus),
                );
                e.insert(collector);
            }
            continue;
        }
        if let Some(bus) = bus_managers.get(&point.bus) {
            let key = point.bus.clone();
            if let std::collections::hash_map::Entry::Vacant(e) = collectors.entry(key) {
                let collector = protocol_registry
                    .create(&point.protocol, &point.bus, bus.clone(), metrics.clone())
                    .ok_or_else(|| anyhow::anyhow!("协议未注册: {}", point.protocol))?;
                e.insert(collector);
            }
        }
    }
    warn_unavailable_bus_points(config, &expanded_points, bus_managers);
    let mut buses_with_collector: Vec<&String> = collectors.keys().collect();
    buses_with_collector.sort();
    info!(
        "created {} collector(s) for bus(es): [{}]",
        collectors.len(),
        buses_with_collector
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );

    Ok((collectors, expanded_points, templates))
}

/// 找出「所属总线没有可用 BusManager」的点位（gpio 除外：它不依赖总线管理器）。
///
/// 这类点位在 `init_collectors` 里会被跳过，导致配置加载成功、界面正常，
/// 但这几个点位**永远采不到数据**。必须显式告警，否则现场只能靠逐点试错定位。
fn points_with_unavailable_bus<'a>(
    points: &'a [PointConfig],
    bus_managers: &HashMap<String, Arc<BusManager>>,
) -> Vec<(&'a str, &'a str)> {
    points
        .iter()
        .filter(|p| p.protocol != "gpio")
        .filter(|p| !bus_managers.contains_key(&p.bus))
        .map(|p| (p.bus.as_str(), p.id.as_str()))
        .collect()
}

/// 汇总告警：按总线聚合，避免 N 条点位刷屏。
fn warn_unavailable_bus_points(
    config: &GatewayConfig,
    points: &[PointConfig],
    bus_managers: &HashMap<String, Arc<BusManager>>,
) {
    let missing = points_with_unavailable_bus(points, bus_managers);
    if missing.is_empty() {
        return;
    }

    // 区分「没配置」和「配置了但打开失败」——两者现场排查方向完全不同
    let configured: std::collections::HashSet<&str> =
        config.buses.iter().map(|b| b.port_name.as_str()).collect();
    let mut by_bus: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for (bus, point) in missing {
        by_bus.entry(bus).or_default().push(point);
    }

    warn!(
        "{} point(s) will never be collected: no open collector for their bus",
        by_bus.values().map(|v| v.len()).sum::<usize>()
    );
    for (bus, points) in by_bus {
        let reason = if configured.contains(bus) {
            "declared in config.buses but failed to open"
        } else {
            "not declared in config.buses"
        };
        warn!(
            "  bus '{}' {}; affected point(s): [{}]",
            bus,
            reason,
            points.join(", ")
        );
    }
}

/// 把 devices 展开成 points（template 模式 → point 模式）。
pub(crate) fn expand_config_points(
    config: &GatewayConfig,
    templates: &Arc<TemplateManager>,
) -> Result<Vec<PointConfig>> {
    let mut all = config.points.clone();
    if let Some(devices) = &config.devices {
        let expanded = templates
            .expand_devices(devices)
            .map_err(|e| anyhow::anyhow!("expand devices failed: {}", e))?;
        all.extend(expanded);
    }
    Ok(all)
}

/// 启动软件看门狗（W1）：每 5 秒检查一次心跳，独立 spawn_catch_unwind 任务。
pub(crate) fn spawn_software_watchdog(metrics: Arc<gateway_core::engine::metrics::Metrics>) {
    let watchdog = gateway_core::engine::monitor::heartbeat::init(
        std::time::Duration::from_secs(5),
        metrics.clone(),
    );
    let wd = watchdog.clone();
    gateway_core::engine::supervision::spawn_catch_unwind("watchdog", async move {
        wd.run().await;
    });
    info!("watchdog started, check interval = 5s");
}

// ============================================================================
// 阶段函数：Step 2-7（采集链初始化）
// ============================================================================

/// Step 2-7：初始化采集链（logging/watchdog/NTP/GPIO/串口总线/采集器）。
///
/// 注意：Step 1（config 加载）保留在主函数，因为 `config_loader` 需供
/// `AppState` 使用；本函数只接收已加载的 `config: &GatewayConfig`。
pub(crate) async fn init_collection_chain(
    config: &GatewayConfig,
    metrics: Arc<gateway_core::engine::metrics::Metrics>,
) -> Result<CollectionChain> {
    // ===== Step 2: 日志 =====
    gateway_core::engine::logging::init_logging(config.telemetry.as_ref());

    // 生效时区必须在日志里可见：计划任务按**墙钟**解释 cron 表达式，
    // 设备时区与客户预期差 8 小时会导致「每天 02:00」整体偏移，且无任何报错。
    info!(
        "device timezone: {}",
        gateway_core::support::cron::tz_description()
    );

    // ===== Step 3: Watchdog =====
    #[cfg(target_os = "linux")]
    {
        let watchdog_hw = Some("/dev/watchdog");
        let watchdog = Watchdog::start(Duration::from_secs(15), watchdog_hw);
        // P0 修复：原实现为 `let _watchdog = Watchdog::start(...)`，该绑定随本块
        // 结束即被 drop；而 `Watchdog` 没有 `Drop` 实现，其监控线程仍持有 Arc 存活，
        // 此后**全进程再无任何 `kick()` 调用**（其余 kick 属另一套 Heartbeat）→
        // 15s 后监控线程必然执行 `std::process::abort()`，进程被 systemd 反复拉起，
        // 形成启动-崩溃循环（仅 Linux 触发，故 Windows 开发机上从未暴露）。
        //
        // 现将句柄移入常驻 task 周期喂狗：只要 tokio 运行时仍在正常调度，进程即存活；
        // 若运行时整体卡死（死锁/调度饥饿），本 task 不再被调度，看门狗仍会在 15s 后
        // 按设计 abort —— 保留了它作为"整机兜底"的能力，同时不再误杀正常进程。
        tokio::spawn(async move {
            // 5s 周期，为 15s 超时留出 3 倍余量，避免调度抖动导致误触发
            let mut ticker = tokio::time::interval(Duration::from_secs(5));
            loop {
                ticker.tick().await;
                watchdog.kick();
            }
        });
        info!("watchdog started (periodic kick every 5s)");
    }
    #[cfg(not(target_os = "linux"))]
    {
        info!("platform watchdog skipped (non-linux)");
    }

    // ===== Step 4: NTP =====
    let ntp_config = config.ntp.clone().unwrap_or_else(|| NtpConfig {
        server: "pool.ntp.org".to_string(),
        sync_interval: 3600,
    });
    let ntp_server = ntp_config.server.clone();
    let ntp = Arc::new(NtpManager::new(ntp_config));
    ntp.start();
    info!("ntp manager started (server={})", ntp_server);

    // ===== Step 5: GPIO =====
    // F-05：原先只 new 不 init，`GpioController::init()` 全仓零调用，
    // 导致 /api/io/status 的 DI 读数恒为假值（cached LineHandle 从未建立）。
    // 采集与 DO 写走的是另一条全局路径，故此项只影响 DI 状态展示。
    // init 失败不阻断启动（无 gpiochip 的开发机/容器很常见，仅降级为无缓存路径）。
    let mut gpio_ctrl = GpioController::new();
    if let Err(e) = gpio_ctrl.init() {
        warn!("gpio init failed (DI status will be unavailable): {}", e);
    }
    let gpio = Arc::new(Mutex::new(gpio_ctrl));
    info!("gpio controller initialized");

    // ===== Step 6: 打开串口总线 =====
    let (_serial_mgr, bus_managers) = open_serial_buses(config, metrics.clone());

    // ===== Step 7: 初始化 Collector（注册表 + 按总线创建） =====
    let (collectors, expanded_points, templates) =
        init_collectors(config, &bus_managers, metrics.clone())?;

    Ok(CollectionChain {
        ntp,
        gpio,
        bus_managers,
        collectors,
        expanded_points,
        templates,
    })
}

#[cfg(test)]
mod tests {
    use super::points_with_unavailable_bus;
    use gateway_core::config::PointConfig;
    use gateway_core::south::bus::BusManager;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn point(id: &str, bus: &str, protocol: &str) -> PointConfig {
        PointConfig {
            id: id.to_string(),
            bus: bus.to_string(),
            protocol: protocol.to_string(),
            ..Default::default()
        }
    }

    /// 回归：总线未配置/打开失败时点位会被跳过，必须能被识别出来用于告警；
    /// gpio 不依赖 BusManager，不应误报。
    #[test]
    fn reports_points_on_missing_bus_and_exempts_gpio() {
        let points = vec![
            point("p_missing", "ghost", "modbus"),
            point("p_other_bus", "bus0", "dlt645"),
            point("gpio_point", "gpio-bus", "gpio"),
        ];
        let buses: HashMap<String, Arc<BusManager>> = HashMap::new();
        let missing = points_with_unavailable_bus(&points, &buses);
        assert_eq!(
            missing,
            vec![("ghost", "p_missing"), ("bus0", "p_other_bus")],
            "非 gpio 的缺失总线点位都要告警"
        );
    }
}
