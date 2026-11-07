//! 协议工具 handler 聚合
//! 路由见 `routes.rs` 各能力分组

pub mod auto_detect;
pub mod iec104_ioa_read;
pub mod packet_capture;
pub mod packet_editor;
pub mod packet_replay;
pub mod protocol_convert;
pub mod protocol_diag;
pub mod register_scan;
pub mod replay_store;
pub mod slave_scan;
pub mod slave_simulator;

pub(crate) use auto_detect::*;
pub(crate) use iec104_ioa_read::*;
pub(crate) use packet_capture::*;
pub(crate) use packet_editor::*;
pub(crate) use packet_replay::*;
pub(crate) use protocol_convert::*;
pub(crate) use protocol_diag::*;
pub(crate) use register_scan::*;
pub(crate) use slave_scan::*;
pub(crate) use slave_simulator::*;

use crate::web::services::protocol_capabilities;
use crate::web::types::AppState;
use axum::extract::State;
use axum::Json;
use std::collections::HashMap;
use std::sync::Arc;

/// 进程内结果表（探测）的容量上限。
///
/// 每次发起探测都会新增一条 entry，旧实现从不移除 → 现场长跑进程
/// 反复探测时内存无限增长。只淘汰**已结束**的条目：运行中的 entry 仍被后台
/// 任务持有（通过 `Arc` 克隆），从表里移除会让前端查不到进度。
pub(crate) const MAX_RESULT_ENTRIES: usize = 64;

/// 按「已结束 + 开始时间升序」淘汰最旧条目，直到 `map.len() <= max`。
///
/// `info` 返回 `(是否已结束, 开始毫秒时间戳)`。
pub(crate) fn prune_finished_results<T>(
    map: &mut HashMap<String, T>,
    max: usize,
    info: impl Fn(&T) -> (bool, i64),
) {
    if map.len() <= max {
        return;
    }
    let mut finished: Vec<(i64, String)> = map
        .iter()
        .filter_map(|(k, e)| {
            let (done, start_ms) = info(e);
            done.then(|| (start_ms, k.clone()))
        })
        .collect();
    finished.sort_by_key(|(ts, _)| *ts);
    let excess = map.len() - max;
    for (_, k) in finished.into_iter().take(excess) {
        map.remove(&k);
    }
}

/// S3: GET /api/protocols — 返回所有协议能力元数据
pub(crate) async fn list_protocols(State(_state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let data = protocol_capabilities();
    Json(serde_json::json!({ "code": 0, "message": "ok", "data": data }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 超限只淘汰「已结束 + 最旧」的条目；运行中的必须保留（否则前端查不到进度）
    #[test]
    fn prune_只淘汰最旧的已结束条目_运行中恒保留() {
        let mut map: HashMap<String, (bool, i64)> = HashMap::new();
        map.insert("run_old".into(), (false, 1));
        map.insert("run_new".into(), (false, 9));
        map.insert("done_1".into(), (true, 2));
        map.insert("done_2".into(), (true, 4));
        map.insert("done_3".into(), (true, 6));

        prune_finished_results(&mut map, 2, |(finished, ts)| (*finished, *ts));

        // 5 → 2：淘汰 3 条已结束（最旧优先），运行中的 2 条全留
        assert_eq!(map.len(), 2);
        assert!(map.contains_key("run_old") && map.contains_key("run_new"));
    }

    #[test]
    fn prune_未超限时不淘汰任何条目() {
        let mut map: HashMap<String, (bool, i64)> = HashMap::new();
        map.insert("a".into(), (true, 1));
        map.insert("b".into(), (true, 2));
        prune_finished_results(&mut map, 4, |(finished, ts)| (*finished, *ts));
        assert_eq!(map.len(), 2);
    }
}
