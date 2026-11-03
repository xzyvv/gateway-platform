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
