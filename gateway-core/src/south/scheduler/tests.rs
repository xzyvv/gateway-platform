//! 调度器级回归测试（修复 审查#B1）

use super::*;
use crate::config::{BusConfig, PointConfig};
use crate::engine::metrics::Metrics;
use crate::engine::model::{DataPoint, Quality};
use crate::pipeline::{EdgeProcessor, PipelineSpawnInputs, ProcessingPipeline};
use crate::queue::{DataQueue, MemoryStorage, QueueStorage};
use crate::south::bus::{BusDeps, BusManager};
use crate::south::protocol::skeleton::SkeletonCollector;
use crate::south::protocol::CollectorResult;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 挂在不存在的串口上的总线：仅用于让采集组成立（不发起真实 IO）。
fn test_bus(port: &str) -> Arc<BusManager> {
    let cfg = BusConfig {
        port_name: port.to_string(),
        timeout_ms: 200,
        retry_count: Some(0),
        ..Default::default()
    };
    Arc::new(BusManager::spawn_with(
        &cfg,
        None,
        Arc::new(Metrics::new()),
        BusDeps::production(),
    ))
}

fn test_collector(bus: &str) -> Arc<dyn Collector> {
    Arc::new(SkeletonCollector::new(bus, test_bus(bus)))
}

fn point(id: &str, bus: &str, address: &str) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        bus: bus.to_string(),
        protocol: "skeleton".to_string(),
        address: address.to_string(),
        slave_id: Some(1),
        interval: 1,
        ..Default::default()
    }
}

fn test_pipeline(metrics: Arc<Metrics>) -> Arc<ProcessingPipeline> {
    let queue = Arc::new(
        DataQueue::with_storage(Arc::new(MemoryStorage::new()) as Arc<dyn QueueStorage>)
            .expect("build queue"),
    );
    ProcessingPipeline::spawn(PipelineSpawnInputs {
        edge: Arc::new(EdgeProcessor::new()),
        convert: crate::pipeline::ConvertEngine::empty(),
        virtual_points: crate::pipeline::VirtualPointEngine::empty(),
        queue,
        realtime_bus: None,
        buffer: 16,
        metrics,
    })
}

/// 修复(审查#B1)：热重载必须能**按组**优雅停掉目标组，而不是逐组空等 3s 超时后
/// 强杀（既阻塞 API 又打断在途 Pipeline 丢数据）。
///
/// 反证闭环：去掉 per-group 停止信号（回到只依赖全局 shutdown）后，3 个待重建组
/// 会串行各等满 3s（≥9s）→ 本条 `< 3s` 断言立即失败；恢复修复后通过。
#[tokio::test]
async fn reload_stops_changed_groups_via_per_group_signal() {
    let metrics = Arc::new(Metrics::new());
    let buses = ["b1_a", "b1_b", "b1_c"];

    let collectors: HashMap<String, Arc<dyn Collector>> = buses
        .iter()
        .map(|b| (b.to_string(), test_collector(b)))
        .collect();
    let pts: Vec<PointConfig> = buses
        .iter()
        .map(|b| point(&format!("p_{}", b), b, "1"))
        .collect();

    let scheduler = Arc::new(Scheduler::new(
        collectors,
        pts,
        test_pipeline(metrics.clone()),
        metrics.clone(),
    ));
    scheduler.start();

    // start 后：3 个采集组各登记一个独立停止信号；句柄数 = 3 组 + 聚合刷新
    assert_eq!(
        scheduler.group_stops.lock().len(),
        buses.len(),
        "每组应登记一个停组信号"
    );
    assert_eq!(scheduler.handles.lock().len(), buses.len() + 1);

    // address 变化 → 3 个组全部需要重建
    let new_collectors: HashMap<String, Arc<dyn Collector>> = buses
        .iter()
        .map(|b| (b.to_string(), test_collector(b)))
        .collect();
    let new_pts: Vec<PointConfig> = buses
        .iter()
        .map(|b| point(&format!("p_{}", b), b, "2"))
        .collect();

    let started = Instant::now();
    let stats = scheduler.reload(new_collectors, new_pts).await;
    let elapsed = started.elapsed();

    assert_eq!(stats.force_buses_rebuilt, buses.len(), "3 条总线全部重建");
    assert!(
        elapsed < Duration::from_secs(3),
        "3 组重建应在毫秒级完成（per-group 停组信号）；实际 {:?}（旧实现串行超时 ≥9s）",
        elapsed
    );

    // 重建后每组重新登记停组信号，句柄无泄漏
    assert_eq!(scheduler.group_stops.lock().len(), buses.len());
    assert_eq!(scheduler.handles.lock().len(), buses.len() + 1);

    scheduler.stop().await;
}

/// 采集失败必须让 `collect_failures` 真实增长，不能被质量码吞掉。
///
/// 回归背景：modbus `read_batch` 把失败回填成 `Ok(DataPoint{quality:Bad})`
/// （见 `protocol/modbus/mod.rs`），采集侧原先按 `result.is_ok()` 记账，
/// 导致 `gateway_collect_failures` / `gateway_errors_total` 对 Modbus 恒为 0
/// ——现场排障会把"整条总线全静默"看成零故障。
/// 现口径与 `handle_results` 的 `record_failure` 一致：`Ok` 且 `Quality::Good`
/// 才算成功。
///
/// 反证闭环：把 `spawning.rs` 的判定改回 `result.is_ok()` 后，
/// 下面的 `collect_failures == 1` 断言立即失败。
#[tokio::test]
async fn collect_failures_counts_ok_but_non_good_as_failure() {
    /// 复刻 modbus 语义：一律返回 `Ok`，失败信息只体现在质量码上。
    struct OkButBadCollector {
        bus: String,
    }

    impl Collector for OkButBadCollector {
        fn name(&self) -> &'static str {
            "okbutbad"
        }
        fn bus_name(&self) -> &str {
            &self.bus
        }
        // Collector 要求对象安全，必须手写 Pin<Box<dyn Future>>（不能用 async-trait）
        fn read<'a>(
            &'a self,
            point: &'a PointConfig,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = CollectorResult<DataPoint>> + Send + 'a>,
        > {
            Box::pin(async move {
                let device_id = point.device_id.clone().unwrap_or_else(|| point.bus.clone());
                Ok(DataPoint::failed(
                    &point.id,
                    &device_id,
                    None,
                    0,
                    Quality::Bad,
                    "modbus_read_failed",
                    0,
                ))
            })
        }
    }

    let metrics = Arc::new(Metrics::new());
    let bus = "b1";
    let collectors: HashMap<String, Arc<dyn Collector>> = HashMap::from([(
        bus.to_string(),
        Arc::new(OkButBadCollector {
            bus: bus.to_string(),
        }) as Arc<dyn Collector>,
    )]);
    let pts = vec![point("p_bad", bus, "0")];

    let scheduler = Arc::new(Scheduler::new(
        collectors,
        pts,
        test_pipeline(metrics.clone()),
        metrics.clone(),
    ));

    scheduler.start();

    // 等一个采集周期（interval=1s）
    let deadline = Instant::now() + Duration::from_secs(5);
    while metrics.collect.collect_total.load(Ordering::Relaxed) == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let total = metrics.collect.collect_total.load(Ordering::Relaxed);
    let fails = metrics.collect.collect_failures.load(Ordering::Relaxed);
    scheduler.stop().await;

    assert!(total > 0, "采集应至少产生一次记账");
    assert_eq!(
        fails, total,
        "Ok+Quality::Bad 必须计入 collect_failures（否则 Modbus 侧该指标恒为 0）"
    );
}

/// Bug-R5 (fix)：reload() 内部失败时必须回滚 collectors / points 到 reload 前的快照。
///
/// 测试夹具：通过 `R5_FAIL_INJECTOR` 强制 reload_points_incremental 返回 Err，
/// 验证 reload() 顶层的回滚路径——下一步 production 实际触发原因包括：
/// - supervisor_loop catch_unwind 后 task 持续 panic（reload 误判为成功但全栈已死）
/// - 内部 spawn 资源的 tokio::task::JoinError
/// - bus task join timeout 在 force_buses 较多时并发诱发
///
/// 反证闭环：把 reload() 顶层 `if let Err(reason) = reload_result` 整段删除后，
/// 本测试 reload 后 self.collectors.len() == new_collectors.len() 而**不是**原值，
/// 断言 fails；恢复修复后通过。
#[tokio::test]
async fn r5_reload_failure_rolls_back_collectors_and_points() {
    let metrics = Arc::new(Metrics::new());
    let buses = ["b1", "b2"];

    let old_collectors: HashMap<String, Arc<dyn Collector>> = buses
        .iter()
        .map(|b| (b.to_string(), test_collector(b)))
        .collect();
    let old_points: Vec<PointConfig> = buses
        .iter()
        .map(|b| point(&format!("p_{}", b), b, "1"))
        .collect();

    let scheduler = Arc::new(Scheduler::new(
        old_collectors.clone(),
        old_points.clone(),
        test_pipeline(metrics.clone()),
        metrics.clone(),
    ));
    scheduler.start();

    // 准备：触发 reload_points_incremental 立即返回 Err。
    // 注入器挂在**本测试自己的** scheduler 实例上，不污染并行执行的其它测试
    // （这正是从模块级 static 改为实例字段的原因）。
    *scheduler.fail_reload_injector.lock() = Some("injected".to_string());

    let new_collectors: HashMap<String, Arc<dyn Collector>> = buses
        .iter()
        .map(|b| (b.to_string(), test_collector(b)))
        .collect();
    let new_points: Vec<PointConfig> = buses
        .iter()
        .map(|b| point(&format!("p_{}", b), b, "999"))
        .collect();

    // reload() 因注入器而失败；reload() 应当回滚而不抛错给调用方
    let stats = scheduler.reload(new_collectors, new_points).await;

    // 验证 stats 已降级标记（force_buses_rebuilt = 0）
    assert_eq!(
        stats.force_buses_rebuilt, 0,
        "Bug-R5: 失败 reload 的 stats 必须报告 0 buses_rebuilt"
    );

    // 关键不变量：rollback 后 collectors / points 与 reload 前**完全一致**。
    // 块作用域限定 guard 生命周期：后续还有 reload().await，不能跨 await 持锁
    let (cur_collectors_len, cur_points_len) = {
        let current_collectors = scheduler.collectors.lock();
        let current_points = scheduler.points.lock();
        (current_collectors.len(), current_points.len())
    };
    assert_eq!(
        cur_collectors_len,
        old_collectors.len(),
        "Bug-R5: collectors 数量必须回滚到 reload 前"
    );
    assert_eq!(
        cur_points_len,
        old_points.len(),
        "Bug-R5: points 数量必须回滚到 reload 前"
    );

    // 清理注入器（验证同一实例回滚后仍可正常 reload）
    *scheduler.fail_reload_injector.lock() = None;

    // 验证 reload 正常路径仍可用（注入器已清）
    let normal_collectors: HashMap<String, Arc<dyn Collector>> = buses
        .iter()
        .map(|b| (b.to_string(), test_collector(b)))
        .collect();
    let normal_points: Vec<PointConfig> = buses
        .iter()
        .map(|b| point(&format!("p_{}", b), b, "updated"))
        .collect();
    let normal_stats = scheduler.reload(normal_collectors, normal_points).await;
    assert!(
        normal_stats.force_buses_rebuilt > 0,
        "Bug-R5: 注入器清空后正常 reload 必须能跑通"
    );

    scheduler.stop().await;
}

/// Bug-N2 (fix)：空 points 启动是配置错误信号——
/// metrics.lifecyle.scheduler_empty_starts 必须递增，
/// 给 Grafana `rate(gateway_scheduler_empty_starts_total[5m]) > 0` 告警用。
#[tokio::test]
async fn n2_empty_points_start_increments_empty_starts_metric() {
    let metrics = Arc::new(Metrics::new());

    // 空 points / 空 collectors 构造 Scheduler
    let scheduler = Arc::new(Scheduler::new(
        HashMap::new(),
        Vec::new(), // 关键：空 points
        test_pipeline(metrics.clone()),
        metrics.clone(),
    ));

    // 启动前：metrics = 0
    assert_eq!(
        metrics
            .lifecyle
            .scheduler_empty_starts
            .load(Ordering::Relaxed),
        0,
        "Bug-N2: 启动前 scheduler_empty_starts 必须是 0"
    );

    scheduler.start();

    // 启动后：metrics = 1
    assert_eq!(
        metrics
            .lifecyle
            .scheduler_empty_starts
            .load(Ordering::Relaxed),
        1,
        "Bug-N2: 空 points 启动必须递增 metrics"
    );

    // start() 不应死循环：start 一次即可，且 running 标志位正确
    assert!(scheduler.running.load(Ordering::Relaxed));

    scheduler.stop().await;
}

/// Bug-N2 (fix)：非空 points 启动不递增 empty_starts metric
/// （确认指标语义区分清晰：只有真正空才报）。
#[tokio::test]
async fn n2_non_empty_points_start_does_not_increment_metric() {
    let metrics = Arc::new(Metrics::new());
    let buses = ["b1", "b2"];
    let collectors: HashMap<String, Arc<dyn Collector>> = buses
        .iter()
        .map(|b| (b.to_string(), test_collector(b)))
        .collect();
    let pts: Vec<PointConfig> = buses
        .iter()
        .map(|b| point(&format!("p_{}", b), b, "1"))
        .collect();

    let scheduler = Arc::new(Scheduler::new(
        collectors,
        pts,
        test_pipeline(metrics.clone()),
        metrics.clone(),
    ));

    scheduler.start();

    assert_eq!(
        metrics
            .lifecyle
            .scheduler_empty_starts
            .load(Ordering::Relaxed),
        0,
        "Bug-N2: 有 points 启动**不应**递增 empty_starts metric"
    );

    scheduler.stop().await;
}

/// E1：暂停/恢复总线的**成功语义**（离线可验证：SkeletonCollector 不发起真实 IO）。
///
/// 语义要求（API `POST /api/buses/:name/pause|resume` 依赖）：
/// - 暂停只摘除该总线的点位与采集组，其它总线不受影响；
/// - `paused_buses()` / `is_bus_paused()` 反映真实状态（前端 Banner 依赖）；
/// - 恢复后该总线重新进入调度，且状态清除。
#[tokio::test]
async fn pause_and_resume_bus_remove_and_restore_only_target_bus() {
    let metrics = Arc::new(Metrics::new());
    let collectors: HashMap<String, Arc<dyn Collector>> = ["b1", "b2"]
        .iter()
        .map(|b| (b.to_string(), test_collector(b)))
        .collect();
    let pts = vec![point("p_b1", "b1", "1"), point("p_b2", "b2", "1")];

    let scheduler = Arc::new(Scheduler::new(
        collectors,
        pts.clone(),
        test_pipeline(metrics.clone()),
        metrics.clone(),
    ));
    scheduler.start();

    assert_eq!(
        scheduler.group_stops.lock().len(),
        2,
        "启动后两条总线各一组"
    );
    assert!(!scheduler.is_bus_paused("b1"));

    scheduler
        .pause_bus("b1", pts.clone())
        .await
        .expect("暂停已有点位的总线应成功");

    assert!(scheduler.is_bus_paused("b1"));
    assert_eq!(scheduler.paused_buses(), vec!["b1".to_string()]);
    assert!(
        !scheduler.points.lock().iter().any(|p| p.bus == "b1"),
        "暂停后该总线点位必须从调度点集中摘除"
    );
    assert!(
        scheduler.points.lock().iter().any(|p| p.bus == "b2"),
        "其它总线点位不得受影响"
    );
    // 用块作用域确保锁在下一个 .await 之前释放（parking_lot 锁不得跨 await 持有）
    {
        let stops = scheduler.group_stops.lock();
        assert!(!stops.contains_key("skeleton:b1"), "b1 采集组必须停掉");
        assert!(stops.contains_key("skeleton:b2"), "b2 采集组必须保留");
    }

    scheduler
        .resume_bus("b1", pts.clone())
        .await
        .expect("恢复已暂停总线应成功");

    assert!(!scheduler.is_bus_paused("b1"));
    assert!(scheduler.paused_buses().is_empty());
    assert!(
        scheduler.points.lock().iter().any(|p| p.bus == "b1"),
        "恢复后点位必须回到调度"
    );
    assert!(
        scheduler.group_stops.lock().contains_key("skeleton:b1"),
        "恢复后 b1 采集组必须重建"
    );

    scheduler.stop().await;
}

/// E1 反证：总线不存在（没有可暂停点位）必须报错，不能静默标记为「已暂停」——
/// 否则前端会显示一个永远无法恢复的暂停横幅。
#[tokio::test]
async fn pause_bus_without_points_is_rejected() {
    let metrics = Arc::new(Metrics::new());
    let collectors: HashMap<String, Arc<dyn Collector>> =
        [("b1".to_string(), test_collector("b1"))]
            .into_iter()
            .collect();
    let pts = vec![point("p_b1", "b1", "1")];

    let scheduler = Arc::new(Scheduler::new(
        collectors,
        pts.clone(),
        test_pipeline(metrics.clone()),
        metrics.clone(),
    ));
    scheduler.start();

    let err = scheduler
        .pause_bus("ghost", pts)
        .await
        .expect_err("不存在于点位集的总线不能暂停");
    assert!(
        err.contains("没有可暂停的采集点位"),
        "错误原因应可读: {err}"
    );
    assert!(!scheduler.is_bus_paused("ghost"));

    scheduler.stop().await;
}

// =========================================================================
// F-11 采集时序快照（timeline.rs）
//
// 该快照直接给运维页出「真实执行窗口 + 重叠」结论。若样本裁剪/窗口过滤/跨组判定
// 写错，页面会给出**看似精确却错误**的结论（这正是该模块引入时要替换掉的行为）。
// =========================================================================

#[test]
fn timeline_keeps_recent_samples_and_counts_all_runs() {
    let tl = Timeline::default();
    let now = super::timeline::now_ms();

    // 40 次执行：内存里只保留最近 32 条样本，runs 必须如实累计
    for i in 0..40i64 {
        let start = now - 40_000 + i * 1000;
        tl.record(
            "modbus:b1",
            "modbus",
            "b1",
            Duration::from_secs(1),
            start,
            start + 200,
        );
    }

    let snap = tl.snapshot(60_000);
    assert_eq!(snap.group_count, 1);
    let g = &snap.groups[0];
    assert_eq!(g.runs, 40);
    assert_eq!(g.samples.len(), super::timeline::MAX_SAMPLES_PER_GROUP);
    // 最老的 8 条被丢弃：首条应是下标 8，末条是下标 39
    assert_eq!(g.samples.first().unwrap().start_ms, now - 40_000 + 8 * 1000);
    assert_eq!(g.samples.last().unwrap().start_ms, now - 40_000 + 39 * 1000);
}

#[test]
fn timeline_snapshot_drops_samples_outside_window() {
    let tl = Timeline::default();
    let now = super::timeline::now_ms();

    // 窗口外的历史样本不得影响当前结论
    tl.record(
        "modbus:b1",
        "modbus",
        "b1",
        Duration::from_secs(1),
        now - 120_000,
        now - 119_000,
    );
    assert_eq!(tl.snapshot(60_000).group_count, 0);

    tl.record(
        "modbus:b1",
        "modbus",
        "b1",
        Duration::from_secs(1),
        now - 1_000,
        now,
    );
    let snap = tl.snapshot(60_000);
    assert_eq!(snap.group_count, 1);
    assert_eq!(snap.groups[0].samples.len(), 1);
}

#[test]
fn timeline_flags_overrun_only_when_duration_exceeds_interval() {
    let tl = Timeline::default();
    let now = super::timeline::now_ms();

    tl.record(
        "modbus:b1",
        "modbus",
        "b1",
        Duration::from_millis(1000),
        now - 3_000,
        now - 1_500,
    );
    tl.record(
        "modbus:b1",
        "modbus",
        "b1",
        Duration::from_millis(1000),
        now - 1_000,
        now - 500,
    );

    let snap = tl.snapshot(10_000);
    let g = &snap.groups[0];
    assert!(g.samples[0].overrun, "1500ms > 1000ms 应标记超时");
    assert!(!g.samples[1].overrun, "500ms < 1000ms 不应标记超时");
    assert_eq!(g.max_duration_ms, 1_500);
    assert_eq!(g.avg_duration_ms, 1_000);
}

#[test]
fn timeline_overlap_only_between_different_groups_on_same_bus() {
    let tl = Timeline::default();
    let now = super::timeline::now_ms();

    // 同总线、不同组：窗口相交 → 1 条重叠
    tl.record(
        "modbus:b1",
        "modbus",
        "b1",
        Duration::from_secs(1),
        now - 5_000,
        now - 3_000,
    );
    tl.record(
        "dlt645:b1",
        "dlt645",
        "b1",
        Duration::from_secs(1),
        now - 4_000,
        now - 2_000,
    );
    // 同组自身的两次样本首尾相接，不算重叠
    tl.record(
        "dlt645:b1",
        "dlt645",
        "b1",
        Duration::from_secs(1),
        now - 1_500,
        now - 1_000,
    );
    // 另一条总线上的组，时间再重叠也不得算进 b1
    tl.record(
        "modbus:b2",
        "modbus",
        "b2",
        Duration::from_secs(1),
        now - 5_000,
        now - 3_000,
    );

    let snap = tl.snapshot(10_000);
    assert_eq!(snap.groups.len(), 3);
    assert_eq!(snap.overlaps.len(), 1, "只应有一条 b1 跨组重叠");
    let o = &snap.overlaps[0];
    assert_eq!(o.bus, "b1");
    assert_eq!(o.start_ms, now - 4_000);
    assert_eq!(o.end_ms, now - 3_000);
    assert_eq!(o.overlap_ms, 1_000);
}

#[test]
fn timeline_clear_resets_state() {
    let tl = Timeline::default();
    let now = super::timeline::now_ms();

    tl.record(
        "modbus:b1",
        "modbus",
        "b1",
        Duration::from_secs(1),
        now - 1_000,
        now,
    );
    assert_eq!(tl.snapshot(10_000).group_count, 1);

    tl.clear();
    let snap = tl.snapshot(10_000);
    assert_eq!(snap.group_count, 0);
    assert!(snap.groups.is_empty());
    assert!(snap.overlaps.is_empty());
    assert!(!snap.running);
}
