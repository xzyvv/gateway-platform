//! P1：协议诊断端点 `GET /api/protocols/:name/diag`
//!
//! ## 为什么需要这个端点
//!
//! `gateway-core` 里 `Collector::diag` 已有 3 个**真实实现**：
//! - `s7comm`：读 SZL 0x0011 / 0x001C（模块标识与订货号）
//! - `bacnet`：发 Who-Is 广播收 I-Am（设备号 / 厂商 / 最大 APDU）
//! - `gpio`：上报芯片节点路径
//!
//! 但此前**生产代码零调用**——能力位 `supports_diag` 下发到了前端却没有任何
//! 入口能触发它，现场遇到「连不上 / 读不到」时只能靠抓包猜。本 handler 把这
//! 条链路接出来：按协议名匹配正在运行的 collector，逐个执行诊断。
//!
//! ## 为什么要先暂停总线
//!
//! 诊断是**对同一条物理链路/同一个 socket 的额外收发**：
//! - `s7comm` 的 `read_szl` 与采集共用 `stream`，有互斥锁不会被写坏，但会挤占
//!   采集周期的时间预算；
//! - `bacnet` 的 `who_is_device` 会先 `drain_socket` 再循环 `recv`，并丢弃所有
//!   非 I-Am 报文——与采集并发时**会把采集的响应吞掉**，造成真实丢数。
//!
//! 因此对未暂停的总线默认**拒绝执行**，复用既有的「调试独占前先暂停采集」约定
//! （`POST /api/buses/:name/pause`），而不是让用户点一下按钮就在后台悄悄丢数。

use std::sync::Arc;

use axum::extract::{Path, State};

use crate::web::error::ApiError;
use crate::web::services::Collector;
use crate::web::types::{ApiResponse, AppState};

/// 单条总线的诊断结果
#[derive(serde::Serialize)]
pub(crate) struct ProtocolDiagEntry {
    /// 总线名
    pub bus: String,
    /// 是否真正执行了诊断（false 时 `detail` 是未执行的原因，不是诊断结论）
    pub executed: bool,
    /// 诊断输出，或未执行的原因
    pub detail: String,
}

/// 按协议聚合的诊断结果
#[derive(serde::Serialize)]
pub(crate) struct ProtocolDiagResult {
    /// 请求的协议名
    pub protocol: String,
    /// 该协议是否有任一运行实例声明了真实诊断实现（能力位 `supports_diag`）
    pub supported: bool,
    /// 每个运行实例一条，按总线名升序（HashMap 迭代无序，排序保证响应稳定）
    pub entries: Vec<ProtocolDiagEntry>,
}

/// GET /api/protocols/:name/diag — 对指定协议的运行实例执行协议诊断
///
/// 协议未在运行（没有任何 collector 使用它）时返回 404：现场最常见的误操作是
/// 「设备还没配好就点诊断」，如实告知比返回一个空列表更有用。
pub(crate) async fn get_protocol_diag(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<ApiResponse<ProtocolDiagResult>, ApiError> {
    let collectors = state.south.scheduler.collectors();

    let mut matched: Vec<(String, Arc<dyn Collector>)> = collectors
        .into_iter()
        .filter(|(_, c)| c.name() == name.as_str())
        .collect();
    if matched.is_empty() {
        return Err(ApiError::not_found(format!(
            "协议 {} 当前没有运行中的采集实例：请先在配置中创建该协议的设备/总线并下发配置后重试",
            name
        )));
    }
    // HashMap 迭代顺序不确定，固定按总线名排序，便于前端展示与运维比对
    matched.sort_by(|a, b| a.0.cmp(&b.0));

    let supported = matched.iter().any(|(_, c)| c.diag_supported());
    let mut entries = Vec::with_capacity(matched.len());

    for (bus, collector) in matched {
        if !collector.diag_supported() {
            entries.push(ProtocolDiagEntry {
                bus,
                executed: false,
                detail: format!(
                    "协议 {} 未实现协议诊断（supports_diag = false），只能用读取/抓包排障",
                    name
                ),
            });
            continue;
        }
        // 未暂停即采集在跑：诊断会与采集循环抢同一链路（bacnet 会丢弃非 I-Am 响应），
        // 拒绝执行而不是偷偷影响采集质量
        if !state.south.scheduler.is_bus_paused(&bus) {
            entries.push(ProtocolDiagEntry {
                bus,
                executed: false,
                detail:
                    "该总线正在采集中：诊断会与采集循环争用同一链路，请先暂停该总线采集后再执行诊断"
                        .into(),
            });
            continue;
        }
        let detail = collector.diag().await;
        entries.push(ProtocolDiagEntry {
            bus,
            executed: true,
            detail,
        });
    }

    Ok(ApiResponse::ok(ProtocolDiagResult {
        protocol: name,
        supported,
        entries,
    }))
}
