//! Web 层服务门面（Facade）
//!
//! 收敛 web 对 north/south 内部模块的直接依赖：handler 只依赖本模块，
//! 底层模块路径或实现变化时（如协议注册表重构、模板引擎迁移）只需改这一处。
//!
//! 注：这里封装的能力均为**无状态纯函数/常量**，故使用门面函数与再导出，
//! 而非 `web::traits` 的 trait 注入——trait 注入是为需要 mock 的有状态依赖
//! （scheduler/queue/gateway/ota）设计的。

use std::time::Duration;

use serde_json::Value;

use gateway_core::config::BusConfig;
use gateway_core::north::template::TemplateEngine;

/// 模板变量表（handler 构造渲染参数用）
pub(crate) use gateway_core::north::template::{ExtraValue, TemplateVars};
/// 协议能力位查询（注册表驱动的调试工具门控：从站扫描 / 寄存器扫描 / 参数探测）。
/// 与下方返回全部描述符的 [`protocol_capabilities`] 区分：本函数按协议名取能力位。
pub(crate) use gateway_core::south::protocol::registry::protocol_capabilities as protocol_capabilities_of;
/// 采集器契约（`Collector::diag` 等），供协议诊断 handler 调用动态实例
pub(crate) use gateway_core::south::protocol::Collector;
/// 协议白名单（来自 south 协议注册表），供设备/测点配置校验使用
pub(crate) use gateway_core::south::protocol::VALID_PROTOCOLS;

/// 队列健康判定（P0-1 的唯一口径）。
///
/// `web/api` 的分层规则不允许直接引用 `gateway_core::queue`（存储内核），
/// 但 `/api/status`、`/api/watchdog/services`、`/api/health/report`、`/health/ready`
/// 必须共用同一套阈值，否则同一队列长度会得出互斥结论。这里按分层规则做门面再导出：
/// 阈值与判定函数只此一份，handler 侧通过 `crate::web::services` 访问。
pub(crate) use gateway_core::queue::{
    capacity_pressure_rows, queue_health, QueueHealth, BACKLOG_WARN_ROWS, BACKPRESSURE_ROWS,
};

/// 构造 Modbus 读请求帧（FC01/02/03/04 通用格式）
///
/// 协议裁剪构建（`--no-default-features` 且未启用 `modbus`）下 core 的
/// `south::protocol::modbus` 会在编译期被剥离，此处退化为等价本地拼帧：
/// 帧格式由 Modbus 规范固定，CRC16 复用 core 的通用实现（不受 feature 影响），
/// 字节与 `modbus::build_read_request` 完全一致，使参数探测 / 从站扫描 /
/// 寄存器扫描在裁剪构建下仍可用，而不是让整个 gateway-api 编译失败。
#[cfg(feature = "modbus")]
pub(crate) fn modbus_read_request_frame(slave: u8, fc: u8, addr: u16, count: u16) -> Vec<u8> {
    gateway_core::south::protocol::modbus::build_read_request(slave, fc, addr, count)
}

#[cfg(not(feature = "modbus"))]
pub(crate) fn modbus_read_request_frame(slave: u8, fc: u8, addr: u16, count: u16) -> Vec<u8> {
    // `[slave][fc][addrH addrL][qtyH qtyL][CRClo CRChi]`
    let mut frame = Vec::with_capacity(8);
    frame.push(slave);
    frame.push(fc);
    frame.extend_from_slice(&addr.to_be_bytes());
    frame.extend_from_slice(&count.to_be_bytes());
    frame.extend_from_slice(&gateway_core::engine::util::crc::crc16(&frame).to_le_bytes());
    frame
}

/// 直连型协议的单点探测后端（mc / fins 寄存器扫描）。
///
/// 这两个协议自建 TCP 连接、不经过总线 task，`DebugBusTrait::exchange` 的裸帧
/// 语义无法复用，故直接构造采集器调用其 `probe_word`（与采集路径同一套
/// 建帧/应答判定）。本枚举仍放在门面层，避免 `web/api` 直接引用
/// `gateway_core::south`（分层规则）。
pub(crate) enum WordProbe {
    #[cfg(feature = "mc")]
    Mc(gateway_core::south::protocol::mc::McCollector),
    #[cfg(feature = "fins")]
    Fins(gateway_core::south::protocol::fins::FinsCollector),
    /// 当前构建未启用该协议 feature（注册表也不会暴露该协议，正常不可达）
    Unsupported,
}

impl WordProbe {
    /// 按总线配置选择探测后端；`timeout_ms` 为单地址读超时（扫描参数）。
    pub(crate) fn new(bus: &BusConfig, timeout_ms: u64) -> Self {
        // mc/fins 均未启用时，唯一使用 timeout 的分支会被 cfg 剔除，
        // 此时该局部变量在 `-D warnings` 下会触发 unused variable。
        #[cfg_attr(
            not(any(feature = "mc", feature = "fins")),
            allow(unused_variables)
        )]
        let timeout = Duration::from_millis(timeout_ms);
        match bus.protocol.as_str() {
            #[cfg(feature = "mc")]
            // 默认端口与 registry 工厂保持一致，避免调试面与采集面连到不同端点
            "mc" => Self::Mc(gateway_core::south::protocol::mc::McCollector::new(
                bus.tcp_addr
                    .clone()
                    .unwrap_or_else(|| "localhost:6000".into()),
                bus.port_name.clone(),
                timeout,
                bus.effective_variant(),
            )),
            #[cfg(feature = "fins")]
            "fins" => Self::Fins(gateway_core::south::protocol::fins::FinsCollector::new(
                bus.tcp_addr
                    .clone()
                    .unwrap_or_else(|| "localhost:9600".into()),
                bus.port_name.clone(),
                timeout,
                bus.effective_variant(),
            )),
            _ => Self::Unsupported,
        }
    }

    /// 探测一个字；`Some(Ok((结束码, 值)))` 表示拿到应答（值 None=结束码非 0）。
    /// `Some(Err(说明))` 表示链路/帧级失败；`None` 表示该构建不支持此协议。
    // 同 `new`：mc/fins 均未启用时三个入参只服务于被 cfg 剔除的分支。
    #[cfg_attr(
        not(any(feature = "mc", feature = "fins")),
        allow(unused_variables)
    )]
    pub(crate) async fn probe(
        &self,
        station: i32,
        area: &str,
        addr: i32,
    ) -> Option<Result<(u16, Option<i32>), String>> {
        match self {
            #[cfg(feature = "mc")]
            Self::Mc(c) => Some(
                c.probe_word(station as u8, area, addr as u32)
                    .await
                    .map_err(|e| e.to_string()),
            ),
            #[cfg(feature = "fins")]
            Self::Fins(c) => Some(
                c.probe_word(station as u8, area, addr as u32)
                    .await
                    .map_err(|e| e.to_string()),
            ),
            Self::Unsupported => None,
        }
    }
}

/// 校验模板能否编译，失败返回错误消息
pub(crate) fn validate_template(template: &str) -> Result<(), String> {
    TemplateEngine::new(template)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 渲染模板，失败返回错误消息
pub(crate) fn render_template(template: &str, vars: &TemplateVars) -> Result<String, String> {
    let engine = TemplateEngine::new(template).map_err(|e| e.to_string())?;
    Ok(engine.render(vars))
}

/// 安装已启用的数据质量规则到处理管道快照（range / rate / nan 的真实执行入口）。
///
/// `web/api` 分层规则不允许直接引用 `gateway_core::pipeline`，故由本门面承接：
/// 每次规则写操作成功后调用一次，使新规则立即对后续采集数据生效。
/// 读取/安装失败只记 error 日志，不回滚已成功的落库写入（网关仍按旧快照运行）。
pub(crate) fn install_quality_rules(store: &gateway_core::config::QualityRuleStore) {
    match store.list_enabled_rules() {
        Ok(rules) => {
            let count = rules.len();
            gateway_core::pipeline::quality_rules::install_rules(rules);
            tracing::info!("quality rules re-installed: {}", count);
        }
        Err(e) => tracing::error!("reload quality rules failed: {}", e),
    }
}

/// 协议描述符（已序列化为 API 响应形状），供 GET /api/protocols 使用。
/// 前端协议下拉/标签/默认超时/地址提示全部由此驱动，后端注册表是唯一来源。
/// 字段保持向后兼容：`name` / `capabilities` 不变，新增 `label` / `transport` /
/// `default_timeout_ms` / `address_hint` / `endpoint_hint` / `variants` / `tag_color`。
pub(crate) fn protocol_capabilities() -> Vec<Value> {
    gateway_core::south::protocol::registry::all_protocol_descriptors()
        .into_iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name,
                "label": d.label,
                "transport": d.transport,
                "default_timeout_ms": d.default_timeout_ms,
                "address_hint": d.address_hint,
                "endpoint_hint": d.endpoint_hint,
                "variants": d
                    .variants
                    .iter()
                    .map(|(value, label)| serde_json::json!({ "value": value, "label": label }))
                    .collect::<Vec<_>>(),
                "tag_color": d.tag_color,
                "capabilities": d.capabilities,
            })
        })
        .collect()
}
