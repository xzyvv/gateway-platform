use super::*;
use crate::config::{BusConfig, GatewayConfig, PointConfig};
use crate::engine::error::GatewayError;
use crate::south::bus::serial::SerialHandle;
use crate::south::bus::transport::SerialOpener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// r36-APP2 回归：配置热重载（ResetSerial 语义）中串口打开失败时，必须保留
/// 该总线**已有的** manager，而不是静默丢弃。
///
/// 旧实现失败即 `continue`，使该总线既不进 `new_managers` 也不进
/// `all_collectors`；随后 `bus_managers` 被整体替换并 drop 旧 manager，
/// 该总线采集静默停止。修复后旧实例应被回退保留，且不计入"已重建"。
#[tokio::test]
async fn reload_buses_keeps_old_manager_when_open_fails() {
    let gw = gateway_runtime_without_buses_for_test();

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

/// ROI-12：prepare 阶段拒绝未知协议时必须**零副作用**——不新增总线、不改点位。
///
/// 旧实现会先落盘再热重载，失败后只能回滚磁盘；现在 prepare 前置于落盘，
/// 因此拒绝路径天然保持磁盘与运行态都为旧配置。
#[tokio::test]
async fn prepare_reload_rejects_unknown_protocol_without_side_effects() {
    let gw = gateway_runtime_without_buses_for_test();
    let old = GatewayConfig::default();
    let new = GatewayConfig {
        buses: vec![BusConfig {
            port_name: "COM_ROI12_UNKNOWN".to_string(),
            protocol: "no-such-protocol".to_string(),
            tcp_addr: None,
            ..BusConfig::default()
        }],
        points: vec![PointConfig {
            id: "p_roi12".to_string(),
            bus: "COM_ROI12_UNKNOWN".to_string(),
            protocol: "no-such-protocol".to_string(),
            address: "1".to_string(),
            interval: 1,
            ..PointConfig::default()
        }],
        ..GatewayConfig::default()
    };

    let err = match gw.prepare_reload(&old, &new, new.points.clone()).await {
        Ok(_) => panic!("未知协议必须在 prepare 阶段被拒绝"),
        Err(e) => e,
    };
    assert!(
        matches!(err, GatewayError::Config(_)),
        "应返回配置错误，实际: {err:?}"
    );

    assert!(gw.bus_names().is_empty(), "prepare 失败不得上岗任何总线");
    assert!(
        gw.scheduler.points().is_empty(),
        "prepare 失败不得改动点位表"
    );
}

/// ROI-12：点位引用不存在的总线时，prepare 直接拒绝。
///
/// 旧实现到 spawn 才 `warn!("no collector for bus")` 后 return，
/// 表现为「配置已生效、该组静默消失」；现在整次保存被拒绝。
#[tokio::test]
async fn prepare_reload_rejects_point_on_missing_bus() {
    let gw = gateway_runtime_without_buses_for_test();
    let old = GatewayConfig::default();
    let new = GatewayConfig {
        points: vec![PointConfig {
            id: "p_ghost".to_string(),
            bus: "ghost-bus".to_string(),
            protocol: "modbus".to_string(),
            address: "1".to_string(),
            interval: 1,
            ..PointConfig::default()
        }],
        ..GatewayConfig::default()
    };

    let err = match gw.prepare_reload(&old, &new, new.points.clone()).await {
        Ok(_) => panic!("缺 collector 的点位组必须在 prepare 阶段拒绝"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("collector"),
        "错误信息应说明缺少 collector，实际: {err}"
    );
    assert!(gw.scheduler.points().is_empty());
}

// ===== U-5 / U-6：热重载串口交接顺序与失败自愈 =====

/// 释放可观测的串口桩：被 drop 时置位 `released`。
///
/// 串口独占资源——旧 `BusManager` 任务被 abort 后，只有它的 future（含
/// `SerialHandle`）真正 drop，端口才算释放。U-5 用它证明 `open_bus(新口)`
/// 发生在旧端口释放**之后**。
struct ReleaseProbePort {
    released: Arc<AtomicBool>,
}

impl AsyncWrite for ReleaseProbePort {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(buf.len()))
    }
    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for ReleaseProbePort {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Poll::Pending
    }
}

impl Drop for ReleaseProbePort {
    fn drop(&mut self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

/// 记录打开顺序、可注入失败的串口打开器。
struct ProbeOpener {
    /// 视为「新配置」的波特率（该值走成功 / 失败分支由 `fail_new` 决定）。
    new_baud: u32,
    fail_new: bool,
    /// 旧端口是否已释放（由 `ReleaseProbePort` 置位）。
    old_released: Arc<AtomicBool>,
    /// 打开新口那一刻读到的 `old_released` 值（U-5 的顺序证据）。
    released_when_new_opened: Arc<StdMutex<Option<bool>>>,
    /// 依次打开的波特率（断言顺序）。
    opens: Arc<StdMutex<Vec<u32>>>,
}

impl SerialOpener for ProbeOpener {
    fn open(&self, bus: &BusConfig) -> std::io::Result<SerialHandle> {
        self.opens
            .lock()
            .expect("opens poisoned")
            .push(bus.baud_rate);
        if bus.baud_rate == self.new_baud {
            *self
                .released_when_new_opened
                .lock()
                .expect("probe poisoned") = Some(self.old_released.load(Ordering::SeqCst));
            if self.fail_new {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "new port unavailable",
                ));
            }
        }
        Ok(SerialHandle::from_stream(ReleaseProbePort {
            released: Arc::new(AtomicBool::new(false)),
        }))
    }
}

fn u5_bus(baud: u32) -> BusConfig {
    BusConfig {
        port_name: "COM_U5_ORDER".to_string(),
        protocol: "modbus".to_string(),
        tcp_addr: None,
        baud_rate: baud,
        ..BusConfig::default()
    }
}

/// U-5：热重载必须先等旧串口任务退出（端口释放）再打开新口。
#[tokio::test]
async fn reload_closes_old_serial_port_before_opening_new_one() {
    let old_released = Arc::new(AtomicBool::new(false));
    let released_when_new_opened = Arc::new(StdMutex::new(None));
    let opens = Arc::new(StdMutex::new(Vec::new()));
    let opener = Arc::new(ProbeOpener {
        new_baud: 115_200,
        fail_new: false,
        old_released: old_released.clone(),
        released_when_new_opened: released_when_new_opened.clone(),
        opens: opens.clone(),
    });
    let gw = gateway_runtime_with_serial_mgr_for_test(SerialManager::with_opener(opener));

    let old_bus = u5_bus(9_600);
    let new_bus = u5_bus(115_200);
    let old = Arc::new(BusManager::spawn_with(
        &old_bus,
        Some(SerialHandle::from_stream(ReleaseProbePort {
            released: old_released.clone(),
        })),
        gw.metrics.clone(),
        crate::south::bus::transport::BusDeps::production(),
    ));
    gw.bus_managers
        .lock()
        .insert(old_bus.port_name.clone(), old);

    let old_cfg = GatewayConfig {
        buses: vec![old_bus],
        ..GatewayConfig::default()
    };
    let new_cfg = GatewayConfig {
        buses: vec![new_bus],
        ..GatewayConfig::default()
    };
    gw.reload_config(&old_cfg, &new_cfg, Vec::new())
        .await
        .expect("串口交接应成功");

    assert_eq!(
        opens.lock().expect("opens poisoned").as_slice(),
        &[115_200],
        "只应打开新串口一次（旧口由注入句柄直接持有，不经过 opener）"
    );
    assert_eq!(
        *released_when_new_opened.lock().expect("probe poisoned"),
        Some(true),
        "open_bus(新口) 必须在旧端口释放之后（close_task 先于 open_bus）"
    );
    assert!(gw.bus_names().contains(&"COM_U5_ORDER".to_string()));
}

/// U-6：commit 阶段新串口打开失败 → 按旧配置自愈并返回 Err。
#[tokio::test]
async fn reload_serial_open_failure_restores_previous_config_and_errors() {
    let old_released = Arc::new(AtomicBool::new(false));
    let released_when_new_opened = Arc::new(StdMutex::new(None));
    let opens = Arc::new(StdMutex::new(Vec::new()));
    let opener = Arc::new(ProbeOpener {
        new_baud: 115_200,
        fail_new: true,
        old_released: old_released.clone(),
        released_when_new_opened: released_when_new_opened.clone(),
        opens: opens.clone(),
    });
    let gw = gateway_runtime_with_serial_mgr_for_test(SerialManager::with_opener(opener));

    let old_bus = u5_bus(9_600);
    let new_bus = u5_bus(115_200);
    let old = Arc::new(BusManager::spawn_with(
        &old_bus,
        Some(SerialHandle::from_stream(ReleaseProbePort {
            released: old_released.clone(),
        })),
        gw.metrics.clone(),
        crate::south::bus::transport::BusDeps::production(),
    ));
    gw.bus_managers
        .lock()
        .insert(old_bus.port_name.clone(), old);

    let old_cfg = GatewayConfig {
        buses: vec![old_bus],
        ..GatewayConfig::default()
    };
    let new_cfg = GatewayConfig {
        buses: vec![new_bus],
        ..GatewayConfig::default()
    };
    let err = gw
        .reload_config(&old_cfg, &new_cfg, Vec::new())
        .await
        .expect_err("新串口打不开时整次热重载必须失败");
    assert!(
        err.to_string().contains("打开失败"),
        "错误信息应说明串口打开失败，实际: {err}"
    );

    // 打开顺序：先尝试新口（失败），再按旧配置重开旧口（自愈）。
    assert_eq!(
        opens.lock().expect("opens poisoned").as_slice(),
        &[115_200, 9_600],
        "新口失败后必须按旧配置重开一次"
    );
    assert!(
        gw.bus_names().contains(&"COM_U5_ORDER".to_string()),
        "自愈后旧总线必须仍在运行态"
    );
    assert!(
        gw.bus_manager("COM_U5_ORDER").is_some(),
        "自愈后应存在可用的旧配置 manager"
    );
    assert!(
        gw.scheduler.collectors().contains_key("COM_U5_ORDER"),
        "自愈后必须重建旧总线 collector"
    );
}
