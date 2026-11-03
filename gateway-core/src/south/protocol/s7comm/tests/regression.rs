//! 回归：单次多 item Read Var 必须按**协商 PDU**分片（而非硬编码 255）。
use super::*;

use std::time::Duration;

use crate::south::protocol::Collector;

// =====================================================================
// 回归：单次多 item Read Var 必须按**协商 PDU** 分片（而非硬编码 255）
// =====================================================================

/// 回归缺陷维度：修复前分片大小硬编码 255，远超协商 PDU（240 → 每请求最多 18 item），
/// 点位稍多即整批被设备拒绝；且分片变小后多 chunk 成为常态，须验证不回填错位。
///
/// 断言：40 个点位在 PDU=240 下被拆成 3 个 Read Var 请求（18/18/4），每个 ≤18；
/// 且 40 个 DataPoint 全部按原下标回填（值 = 1000 + 下标）。
#[tokio::test]
async fn read_batch_chunks_by_negotiated_pdu_and_maps_by_index() {
    let (addr, req_items) = spawn_s7_stub_pdu240().await;
    let c = S7CommCollector::new(addr, "s7-bus".into(), Duration::from_secs(5));
    let pts: Vec<PointConfig> = (0..40)
        .map(|i| s7_point(&format!("p{i}"), &format!("DB{}.DBD0", i + 1), "dword"))
        .collect();

    let rs = c.read_batch(&pts).await;
    assert_eq!(rs.len(), 40, "返回值必须与入参等长");

    let counts = req_items.lock().unwrap().clone();
    assert_eq!(
        counts.len(),
        3,
        "PDU=240（max_items=18）下 40 点位应拆成 3 个 Read Var 请求，实际收到 {} 个: {:?}",
        counts.len(),
        counts
    );
    assert!(
        counts.iter().all(|n| *n <= 18),
        "每个请求 item 数必须 ≤18（协商 PDU 允许上限），实际 {:?}",
        counts
    );
    assert_eq!(counts.iter().sum::<usize>(), 40);

    for (i, r) in rs.iter().enumerate() {
        let dp = r
            .as_ref()
            .unwrap_or_else(|e| panic!("point {i} err: {e:?}"));
        assert_eq!(dp.point_id, format!("p{i}"), "点位顺序错位");
        assert_eq!(
            dp.raw_value as u64,
            (1000 + i) as u64,
            "point {i} 值错位（应为 {}）",
            1000 + i
        );
    }
}
