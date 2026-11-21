use super::*;
use crate::config::AggregationConfig;

fn make_point(id: &str, deadband: f64, on_change: bool) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        device_id: None,
        name: None,
        bus: "b".into(),
        protocol: "modbus".into(),
        address: "1".into(),
        slave_id: Some(1),
        address_num: Some(0),
        data_id: None,
        function_code: Some(3),
        data_type: Some("u16".into()),
        byte_order: None,
        register_count: Some(1),
        interval: 1,
        timeout_ms: None,
        scale: 1.0,
        offset: 0.0,
        formula: None,
        ct_ratio: None,
        pt_ratio: None,
        deadband,
        on_change,
        unit: None,
        aggregation: None,
        alarm_high: None,
        alarm_low: None,
        modbus_register: None,
        writable: false,
        write_min: None,
        write_max: None,
    }
}

#[test]
fn test_deadband_filter() {
    let ep = EdgeProcessor::new();
    let p = make_point("p1", 1.0, false);
    let dp1 = DataPoint::good("p1", "dev", 10.0, 10.0, "u16");
    assert!(ep.process(&p, dp1).is_some());
    let dp2 = DataPoint::good("p1", "dev", 10.5, 10.5, "u16");
    assert!(ep.process(&p, dp2).is_none());
    let dp3 = DataPoint::good("p1", "dev", 12.0, 12.0, "u16");
    assert!(ep.process(&p, dp3).is_some());
}

#[test]
fn test_on_change() {
    let ep = EdgeProcessor::new();
    let p = make_point("p2", 0.0, true);
    let dp1 = DataPoint::good("p2", "dev", 10.0, 10.0, "u16");
    assert!(ep.process(&p, dp1).is_some());
    let dp2 = DataPoint::good("p2", "dev", 10.0, 10.0, "u16");
    assert!(ep.process(&p, dp2).is_none());
}

#[test]
fn test_bad_quality_passthrough() {
    let ep = EdgeProcessor::new();
    let p = make_point("p3", 10.0, false);
    let dp = DataPoint::failed("p3", "", None, 0, Quality::Bad, "", 0);
    assert!(ep.process(&p, dp).is_some());
}

/// 质量恢复边沿：Bad → Good 的首个样本必须上行一次，即使数值没有变化。
///
/// Bad 样本绕过管道，死区/变化节点的基线仍是故障前的值；若不强制放行，
/// 恢复后数值相同的样本会被当成「未变化」过滤，云端会一直停留在 Bad 状态。
#[test]
fn test_quality_recovery_forces_upload() {
    let ep = EdgeProcessor::new();
    let p = make_point("p7", 1.0, true);

    assert!(ep
        .process(&p, DataPoint::good("p7", "dev", 10.0, 10.0, "u16"))
        .is_some());
    // 值未变 → 死区过滤
    assert!(ep
        .process(&p, DataPoint::good("p7", "dev", 10.0, 10.0, "u16"))
        .is_none());
    // 采集中断：Bad 样本始终上行
    assert!(ep
        .process(
            &p,
            DataPoint::failed("p7", "dev", Some(10.0), 1_000, Quality::Bad, "timeout", 1)
        )
        .is_some());
    // 恢复后的首个 Good（数值仍为 10.0）必须上行
    assert!(ep
        .process(&p, DataPoint::good("p7", "dev", 10.0, 10.0, "u16"))
        .is_some());
    // 之后恢复常规死区过滤
    assert!(ep
        .process(&p, DataPoint::good("p7", "dev", 10.0, 10.0, "u16"))
        .is_none());
}

/// `on_change` 单独接入管道时也按死区判变化，而不是退化为 `f64::EPSILON`。
#[test]
fn test_on_change_uses_deadband_threshold() {
    let ep = EdgeProcessor::with_nodes(vec![Box::new(ChangeNode::new())]);
    let p = make_point("p8", 0.5, true);

    assert!(ep
        .process(&p, DataPoint::good("p8", "dev", 10.0, 10.0, "u16"))
        .is_some());
    assert!(
        ep.process(&p, DataPoint::good("p8", "dev", 10.2, 10.2, "u16"))
            .is_none(),
        "0.2 < 死区 0.5，应过滤"
    );
    assert!(
        ep.process(&p, DataPoint::good("p8", "dev", 10.6, 10.6, "u16"))
            .is_some(),
        "0.6 > 死区 0.5，应上行"
    );
}

#[test]
fn test_pending_queue_no_loss() {
    let ep = EdgeProcessor::new();
    let mut p = make_point("p4", 0.0, false);
    p.aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Last,
    });

    let mk_dp = |val: f64, ts: i64| DataPoint {
        point_id: "p4".to_string(),
        value: val,
        raw_value: val,
        raw_type: "u16".to_string(),
        quality: Quality::Good,
        timestamp: ts,
        device_id: String::new(),
        quality_reason: String::new(),
        source_ts: 0,
        stale_count: 0,
        value_text: None,
    };

    assert!(ep.process(&p, mk_dp(1.0, 0)).is_none());
    assert!(ep.process(&p, mk_dp(2.0, 500)).is_none());
    assert!(ep.process(&p, mk_dp(3.0, 1500)).is_none());
    assert!(ep.process(&p, mk_dp(4.0, 2500)).is_none());
    assert!(ep.process(&p, mk_dp(5.0, 3500)).is_none());

    let result = ep.flush_all(&[p]);
    assert_eq!(result.len(), 4, "应有 4 个聚合点（3 pending + 1 当前）");

    let values: Vec<f64> = result.iter().map(|d| d.value).collect();
    assert_eq!(values, vec![2.0, 3.0, 4.0, 5.0]);
}

/// 验证自定义管道：仅死区，无变化上传
#[test]
fn test_custom_pipeline() {
    let nodes: Vec<Box<dyn PipelineNode>> = vec![Box::new(DeadbandNode::new())];
    let ep = EdgeProcessor::with_nodes(nodes);
    let p = make_point("p5", 0.0, true); // on_change=true 但管道无 ChangeNode
    let dp1 = DataPoint::good("p5", "dev", 10.0, 10.0, "u16");
    assert!(ep.process(&p, dp1).is_some());
    let dp2 = DataPoint::good("p5", "dev", 10.0, 10.0, "u16");
    // 无 ChangeNode，相同值也上传
    assert!(ep.process(&p, dp2).is_some());
}

/// 验证聚合节点作为标准 PipelineNode 接入统一节点循环
///
/// 重构后 AggregationNode 不再被 EdgeProcessor 特殊对待，
/// 而是作为 nodes 列表的首项参与统一 process / flush 流程。
/// 此测试确保行为与原 EdgeProcessor.aggregation 字段一致。
#[test]
fn test_aggregation_in_pipeline_loop() {
    let ep = EdgeProcessor::new();
    let mut p = make_point("p6", 0.0, false);
    p.aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Last,
    });

    // 三个不同窗口的样本：t=0/500 属窗口 0，t=1500 属窗口 1，t=2500 属窗口 2
    let mk_dp = |val: f64, ts: i64| DataPoint {
        point_id: "p6".to_string(),
        value: val,
        raw_value: val,
        raw_type: "u16".to_string(),
        quality: Quality::Good,
        timestamp: ts,
        device_id: String::new(),
        quality_reason: String::new(),
        source_ts: 0,
        stale_count: 0,
        value_text: None,
    };

    // 聚合节点吃掉 Good 样本，process 全部返回 None
    assert!(ep.process(&p, mk_dp(1.0, 0)).is_none());
    assert!(ep.process(&p, mk_dp(2.0, 500)).is_none());
    assert!(ep.process(&p, mk_dp(3.0, 1500)).is_none());
    assert!(ep.process(&p, mk_dp(4.0, 2500)).is_none());

    // flush_all 输出 2 个 pending（窗口 0/1）+ 1 个当前缓冲（窗口 2）
    let result = ep.flush_all(&[p.clone()]);
    assert_eq!(result.len(), 3, "应有 3 个聚合点（2 pending + 1 当前）");

    // 契约（E 批）：聚合点是派生值，`source_ts` 取窗口右边界并与 `timestamp` 一致。
    // 旧实现填 0，北向 OPC UA 会把它映射成 1970-01-01 的 source timestamp。
    assert_eq!(result[0].timestamp, 1000, "窗口 0 的右边界");
    assert_eq!(result[1].timestamp, 2000, "窗口 1 的右边界");
    for dp in &result {
        assert_eq!(
            dp.source_ts, dp.timestamp,
            "聚合点 source_ts 必须等于窗口右边界"
        );
        assert!(dp.source_ts > 0, "聚合点 source_ts 不得为 0");
    }

    // flush_all 后缓冲清空，再次 flush 应返回空
    let again = ep.flush_aggregated(&[p]);
    assert!(again.is_empty(), "flush_all 后再次 flush 应为空");
}

/// 回归测试（聚合下标错配修复）：flush 产出的聚合值必须能走出管道。
///
/// `south/scheduler/spawning.rs` 的聚合刷新任务：原实现把聚合输出与完整点位表按下标
/// `zip` 配对（长度/顺序均无对应关系 → 配置错配）。修复后按 `point_id` 配对，并以
/// `CollectedBatch::flushed` 标记提交，使 `process_batch` 走
/// `EdgeProcessor::process_flushed`——聚合节点在该路径直接放行，不再把已完成的聚合值
/// 重新塞回窗口缓冲（否则当轮丢弃，下一轮 flush 再输出，形成每秒重复上报同一值）。
///
/// 本测试固化两条契约：(1) 常规 `process` 下聚合值会被重新入窗（故 flush 产物必须走
/// 独立路径）；(2) `process_flushed` 下聚合值能正常通过后续死区/变化上传节点。
#[test]
fn test_flushed_value_passes_through_process_flushed() {
    let ep = EdgeProcessor::new();
    let mut p = make_point("p7", 0.0, false);
    p.aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Last,
    });

    let sample = |ts: i64| DataPoint {
        point_id: "p7".to_string(),
        device_id: String::new(),
        value: 7.0,
        raw_value: 7.0,
        raw_type: "u16".to_string(),
        quality: Quality::Good,
        quality_reason: String::new(),
        timestamp: ts,
        source_ts: 0,
        stale_count: 0,
        value_text: None,
    };

    assert!(
        ep.process(&p, sample(0)).is_none(),
        "聚合模式下原始值不直接上传"
    );

    let flushed = ep.flush_all(&[p.clone()]);
    assert_eq!(flushed.len(), 1, "当前窗口应输出 1 个聚合值");
    assert_eq!(flushed[0].raw_type, "aggregated");

    // 契约 1：走常规 process（aggregation 仍在）→ 聚合节点重新入窗，返回 None
    assert!(
        ep.process(&p, flushed[0].clone()).is_none(),
        "常规 process 下聚合值会被重新入窗，故 flush 产物必须走独立路径"
    );

    // 契约 2：走 process_flushed（spawning.rs 的实际路径）→ 正常放行
    assert!(
        ep.process_flushed(&p, flushed[0].clone()).is_some(),
        "process_flushed 下聚合值应能通过死区/变化上传节点"
    );
}

/// 构造指定时间戳的聚合样本。
fn agg_sample(point_id: &str, val: f64, ts: i64) -> DataPoint {
    DataPoint {
        point_id: point_id.to_string(),
        device_id: String::new(),
        value: val,
        raw_value: val,
        raw_type: "u16".to_string(),
        quality: Quality::Good,
        quality_reason: String::new(),
        timestamp: ts,
        source_ts: 0,
        stale_count: 0,
        value_text: None,
    }
}

/// 回归测试（Bug 168）：写入路径（`add_to_buffer`）的补窗必须有迭代上限。
///
/// 原实现只在 `flush_aggregated` 加了 `MAX_CATCHUP_WINDOWS`，`add_to_buffer` 的
/// 空窗补全是无界 `while`。当样本时间戳跨越多个窗口（设备离线后首个恢复样本、
/// 系统时钟前跳、点位临时禁用后重新启用）时，会把整个缺口逐窗 push 进 `pending`，
/// 并在下一次 flush 时逐条展开成 DataPoint —— 内存放大倍数等于缺口窗口数，
/// 且每个点位各算一份。
///
/// 本测试以 1s 窗口、200 个窗口的缺口验证：补齐数量被截断到上限，而非全量生成。
#[test]
fn test_add_to_buffer_catch_up_is_capped() {
    const MAX_CATCHUP_WINDOWS: usize = 64;

    let ep = EdgeProcessor::new();
    let mut p = make_point("p8", 0.0, false);
    p.aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Last,
    });

    // 首个样本落在窗口 0
    assert!(ep.process(&p, agg_sample("p8", 1.0, 0)).is_none());
    // 第二个样本跳到窗口 200：缺口 = 199 个中间空窗，远超上限
    assert!(ep.process(&p, agg_sample("p8", 2.0, 200_000)).is_none());

    let result = ep.flush_all(&[p]);

    let empty_windows = result
        .iter()
        .filter(|d| d.raw_type == "empty_window")
        .count();
    assert_eq!(
        empty_windows, MAX_CATCHUP_WINDOWS,
        "补窗必须截断到 MAX_CATCHUP_WINDOWS（{}），实际生成 {} 个空窗",
        MAX_CATCHUP_WINDOWS, empty_windows
    );

    // 1 个旧窗口聚合值 + 64 个空窗占位 + 1 个当前窗口聚合值
    assert_eq!(result.len(), MAX_CATCHUP_WINDOWS + 2);
    assert_eq!(
        result.iter().filter(|d| d.raw_type == "aggregated").count(),
        2,
        "旧窗口与当前窗口的聚合值都应保留"
    );
}

/// 反向守卫（Bug 168）：缺口小于上限时，补窗必须逐窗完整生成，不能被误截断。
///
/// 该用例锁定「上限只做保护、不改变正常语义」：1s 窗口跨 5 个窗口的缺口
/// 应产出 4 个空窗占位（窗口 1/2/3/4），一个不多一个不少。
#[test]
fn test_add_to_buffer_catch_up_below_cap_is_exact() {
    let ep = EdgeProcessor::new();
    let mut p = make_point("p9", 0.0, false);
    p.aggregation = Some(AggregationConfig {
        window: 1,
        function: AggFn::Last,
    });

    assert!(ep.process(&p, agg_sample("p9", 1.0, 0)).is_none());
    assert!(ep.process(&p, agg_sample("p9", 2.0, 5_000)).is_none());

    let result = ep.flush_all(&[p]);

    let empty_windows: Vec<i64> = result
        .iter()
        .filter(|d| d.raw_type == "empty_window")
        .map(|d| d.timestamp)
        .collect();
    assert_eq!(
        empty_windows,
        vec![2_000, 3_000, 4_000, 5_000],
        "缺口 4 个窗口应精确补齐 4 个占位样本（时间戳为窗口右边界）"
    );

    // E 批：空窗占位样本同样带得出采集时刻（取窗口右边界），不能是 0——
    // 北向 OPC UA 对 0 会映射成「无原始时标」，现场就看不出这个空洞发生在哪一刻。
    for dp in result.iter().filter(|d| d.raw_type == "empty_window") {
        assert_eq!(
            dp.source_ts, dp.timestamp,
            "空窗占位 source_ts 必须等于窗口右边界"
        );
        assert!(dp.source_ts > 0);
    }
}
