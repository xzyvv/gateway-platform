//! 配置分节的热更新能力判定。
//!
//! 用途：Web 契约层在保存/重载配置后如实告知调用方「哪些分节已在线生效、哪些只落盘、
//! 必须重启网关才生效」。此前只有一个 `Vec<String>` 说明列表，未覆盖的分节（webhook /
//! opcua / modbus_server / ...）保存成功却没有任何提示，现场容易误判为已生效。
//!
//! # 维护约定
//!
//! 本模块必须与 [`crate::runtime::GatewayRuntime::reload_config`] 的实际覆盖保持一致：
//! - 在 `reload_config` 新增在线应用逻辑时，把对应分节从 `restart_required` 移到 `applied`；
//! - 新增顶层配置分节时，必须在 [`reload_scope`] 中显式归类（热更 / 需重启 / 纯元数据），
//!   并补齐本文件底部单测，避免出现「保存成功但既不生效也无提示」的静默分节。
//!
//! 纯元数据分节（`device_groups` / `device_assets` / `license`）没有运行时消费者，
//! 不进入任一列表。
//!
//! 注意：`virtual_points` **不是**纯元数据——它由 core 的虚拟点位引擎消费，
//! 公式改动可在运行中热重载（引擎内 `ArcSwap` 原子替换规则集），故归入 `applied`。

use serde::Serialize;

use super::{GatewayConfig, WebConfig};

/// 热更新范围。
///
/// `applied` / `restart_required` 均为稳定的英文分节 key，供 HTTP 契约与前端映射文案；
/// key 一旦发布不要重命名（外部客户端可能依赖）。
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct ReloadScope {
    /// 已在线应用（无需重启）的分节
    pub applied: Vec<&'static str>,
    /// 已写入磁盘、但需重启网关进程才生效的分节
    pub restart_required: Vec<&'static str>,
}

impl ReloadScope {
    /// 两份配置无差异，或仅改动了纯元数据分节
    pub fn is_empty(&self) -> bool {
        self.applied.is_empty() && self.restart_required.is_empty()
    }
}

/// Web 配置中「改了必须重启」的字段判定。
///
/// `api_token` / `username` / `password_hash` / `allow_anon_read` / `allow_default_token` /
/// `ota_signing_key` / `users` / `realtime_max_subscribers` 由鉴权中间件与各 handler 从
/// `state.config` 动态读取，可即时生效；
/// 其余字段在启动期被固化：监听 socket、CORS 中间件、限流器、实时广播缓冲容量。
fn web_needs_restart(old: Option<&WebConfig>, new: Option<&WebConfig>) -> bool {
    let (old, new) = match (old, new) {
        (Some(o), Some(n)) => (o, n),
        (None, None) => return false,
        // 分节整体启用/停用：运行时行为等价于全部字段变化
        _ => return true,
    };
    old.listen_addr != new.listen_addr
        || old.cors_origins != new.cors_origins
        || old.rate_limit_per_sec != new.rate_limit_per_sec
        || old.realtime_buffer != new.realtime_buffer
}

/// 计算 old → new 的热更新范围。
pub fn reload_scope(old: &GatewayConfig, new: &GatewayConfig) -> ReloadScope {
    let mut scope = ReloadScope::default();

    // ===== 在线应用（与 GatewayRuntime::reload_config 覆盖一致）=====
    if old.buses != new.buses {
        scope.applied.push("buses");
    }
    if old.points != new.points {
        scope.applied.push("points");
    }
    if old.devices != new.devices {
        scope.applied.push("devices");
    }
    if old.mqtt != new.mqtt {
        scope.applied.push("mqtt");
    }
    if old.ntp != new.ntp {
        scope.applied.push("ntp");
    }
    if old.web != new.web {
        if web_needs_restart(old.web.as_ref(), new.web.as_ref()) {
            scope.restart_required.push("web");
        } else {
            scope.applied.push("web");
        }
    }
    // 虚拟点位：公式改动由 core 的虚拟点位引擎在线生效（ArcSwap 替换规则集）
    if old.virtual_points != new.virtual_points {
        scope.applied.push("virtual_points");
    }
    // 协议转换规则：与虚拟点位同一机制，由 GatewayRuntime::reload_config 在线替换规则集
    if old.convert_rules != new.convert_rules {
        scope.applied.push("convert_rules");
    }

    // ===== 仅落盘，需重启进程 =====
    // 这些分节在 bootstrap 阶段一次性 spawn（见 gatewayd/bootstrap），运行中没有重载路径。
    if old.webhook != new.webhook {
        scope.restart_required.push("webhook");
    }
    if old.opcua != new.opcua {
        scope.restart_required.push("opcua");
    }
    if old.modbus_server != new.modbus_server {
        scope.restart_required.push("modbus_server");
    }
    if old.modbus_passthrough != new.modbus_passthrough {
        scope.restart_required.push("modbus_passthrough");
    }
    if old.cellular != new.cellular {
        scope.restart_required.push("cellular");
    }
    if old.telemetry != new.telemetry {
        scope.restart_required.push("telemetry");
    }
    if old.self_heal != new.self_heal {
        scope.restart_required.push("self_heal");
    }
    // 模拟消费者：配置文件的改动没有热重载路径——`enabled` 只在 bootstrap 阶段
    // 决定「是否自动拉起」，运行参数只在「下次 start」时读取，故配置改动仍需重启。
    // （运行期的启停与参数修改走 API，不经配置文件热重载，故不受此归类影响。）
    if old.sim_consumer != new.sim_consumer {
        scope.restart_required.push("sim_consumer");
    }
    // 队列行数上限：健康判定每次请求现取配置（立即生效），但存储层淘汰线在
    // 构造时固定，完整生效需要重启。按「任一消费者未生效即需重启」归入此处，
    // 避免现场误以为改了上限就已经开始按新线淘汰。
    if old.queue != new.queue {
        scope.restart_required.push("queue");
    }

    // 纯元数据：device_groups / device_assets / license / schema_version
    // 无运行时消费者，不列入任一列表（落盘即完成）。
    scope
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        CellularConfig, ModbusPassthroughConfig, ModbusServerConfig, OpcuaConfig, SelfHealConfig,
        TelemetryConfig, WebhookConfig,
    };

    fn base() -> GatewayConfig {
        GatewayConfig::default()
    }

    #[test]
    fn hot_reloadable_sections_reported_as_applied() {
        let old = base();
        let mut new = old.clone();
        new.ntp = None;
        let scope = reload_scope(&old, &new);
        assert_eq!(scope.applied, vec!["ntp"]);
        assert!(scope.restart_required.is_empty());
    }

    #[test]
    fn startup_only_sections_reported_as_restart_required() {
        let old = base();
        let mut new = old.clone();
        new.webhook = Some(WebhookConfig::default());
        new.opcua = Some(OpcuaConfig::default());
        new.modbus_server = Some(ModbusServerConfig::default());
        new.modbus_passthrough = Some(ModbusPassthroughConfig::default());
        new.cellular = Some(CellularConfig::default());
        new.telemetry = Some(TelemetryConfig::default());
        new.self_heal = Some(SelfHealConfig::default());

        let scope = reload_scope(&old, &new);
        assert!(scope.applied.is_empty());
        assert_eq!(
            scope.restart_required,
            vec![
                "webhook",
                "opcua",
                "modbus_server",
                "modbus_passthrough",
                "cellular",
                "telemetry",
                "self_heal",
            ]
        );
    }

    /// 模拟消费者是启动期一次性 spawn 的通道：改了必须重启，故归入 restart_required。
    #[test]
    fn sim_consumer_change_requires_restart() {
        let old = base();
        let mut new = old.clone();
        new.sim_consumer.enabled = true;

        let scope = reload_scope(&old, &new);
        assert!(scope.applied.is_empty());
        assert_eq!(scope.restart_required, vec!["sim_consumer"]);
    }

    /// 队列行数上限：淘汰线在存储层构造时固定，改配置要重启才完整生效。
    #[test]
    fn queue_max_rows_change_requires_restart() {
        let old = base();
        let mut new = old.clone();
        new.queue.max_rows = 500_000;

        let scope = reload_scope(&old, &new);
        assert!(scope.applied.is_empty());
        assert_eq!(scope.restart_required, vec!["queue"]);
    }

    /// 实时 WS 并发上限由升级流程现取配置，改完即时对新连接生效。
    #[test]
    fn realtime_max_subscribers_change_is_applied_online() {
        let mut old = base();
        old.web = Some(WebConfig::default());
        let mut new = old.clone();
        new.web.as_mut().unwrap().realtime_max_subscribers = 32;

        let scope = reload_scope(&old, &new);
        assert_eq!(scope.applied, vec!["web"]);
        assert!(scope.restart_required.is_empty());
    }

    #[test]
    fn web_token_change_is_online_but_listen_addr_needs_restart() {
        let mut old = base();
        old.web = Some(WebConfig::default());

        let mut new = old.clone();
        new.web.as_mut().unwrap().api_token = "new-token".into();
        let scope = reload_scope(&old, &new);
        assert_eq!(scope.applied, vec!["web"]);
        assert!(scope.restart_required.is_empty());

        let mut new2 = old.clone();
        new2.web.as_mut().unwrap().listen_addr = "0.0.0.0:9999".into();
        let scope2 = reload_scope(&old, &new2);
        assert!(scope2.applied.is_empty());
        assert_eq!(scope2.restart_required, vec!["web"]);
    }

    #[test]
    fn metadata_only_sections_are_silent() {
        let old = base();
        let mut new = old.clone();
        new.device_groups = Some(Vec::new());
        new.device_assets = Some(Vec::new());
        let scope = reload_scope(&old, &new);
        assert!(scope.is_empty());
    }

    #[test]
    fn virtual_points_change_is_applied_online() {
        // 虚拟点位由 core 引擎在线消费（公式改动即时生效），不是纯元数据
        let old = base();
        let mut new = old.clone();
        new.virtual_points = Some(Vec::new());
        let scope = reload_scope(&old, &new);
        assert_eq!(scope.applied, vec!["virtual_points"]);
        assert!(scope.restart_required.is_empty());
    }

    #[test]
    fn convert_rules_change_is_applied_online() {
        // 协议转换规则与虚拟点位同机制，由 reload_config 在线替换规则集
        let old = base();
        let mut new = old.clone();
        new.convert_rules = Some(Vec::new());
        let scope = reload_scope(&old, &new);
        assert_eq!(scope.applied, vec!["convert_rules"]);
        assert!(scope.restart_required.is_empty());
    }

    /// 守护测试：配置里的每个顶层分节都必须显式归类。
    ///
    /// 之前 convert_rules 漏了归类，出现「实际已热更，但接口既不报 applied 也不报重启」的静默生效。
    /// 这里以 serde 序列化后的顶层键为基准比对归类清单：新增分节若不同步登记，本用例直接失败。
    #[test]
    fn every_config_section_is_classified_in_reload_scope() {
        // applied = 热更；restart = 需重启；silent = 纯元数据/内部字段（不提示）
        // web 为分字段生效：动态项 applied，监听地址/CORS/限流/实时缓冲为 restart
        const CLASSIFIED: &[(&str, &str)] = &[
            ("schema_version", "silent"),
            ("buses", "applied"),
            ("points", "applied"),
            ("point_trash", "silent"),
            ("convert_rules", "applied"),
            ("devices", "applied"),
            ("mqtt", "applied"),
            ("sim_consumer", "restart"),
            ("ntp", "applied"),
            ("web", "applied/restart"),
            ("webhook", "restart"),
            ("cellular", "restart"),
            ("opcua", "restart"),
            ("modbus_server", "restart"),
            ("modbus_passthrough", "restart"),
            ("device_groups", "silent"),
            ("device_assets", "silent"),
            ("virtual_points", "applied"),
            ("telemetry", "restart"),
            ("license", "silent"),
            ("self_heal", "restart"),
            ("queue", "restart"),
        ];

        let json = serde_json::to_value(base()).expect("配置应可序列化");
        let obj = json.as_object().expect("配置顶层应为对象");

        // 注意：Option 分节在 None 时不会出现在序列化结果里（如未启用的 webhook、convert_rules），
        // 因此这里只保证「已出现的分节都有归类」；清单中残留已删除分节需人工清理。
        for key in obj.keys() {
            assert!(
                CLASSIFIED.iter().any(|(k, _)| k == key),
                "配置分节 `{}` 未在 reload_scope 归类：新增分节必须显式选择热更/重启/静默",
                key
            );
        }
    }
}
