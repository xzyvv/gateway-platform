use super::*;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 测试用 connector：累加计数；`fail` 时模拟投递失败（错误只应被记 warn）。
struct TestConnector {
    name_str: String,
    count: AtomicUsize,
    fail: bool,
}

#[async_trait::async_trait]
impl crate::north::NorthConnector for TestConnector {
    fn name(&self) -> &str {
        &self.name_str
    }
    async fn start(&self) -> crate::engine::error::GatewayResult<()> {
        Ok(())
    }
    async fn stop(&self) {}
    async fn publish_batch(&self, _batch: &[DataPoint]) -> crate::engine::error::GatewayResult<()> {
        self.count.fetch_add(1, Ordering::Relaxed);
        if self.fail {
            // 失败 connector：每次投递都报错，验证不影响其它 connector
            return Err(crate::engine::error::GatewayError::Protocol(
                "injected publish failure".into(),
            ));
        }
        Ok(())
    }
    fn health(&self) -> crate::north::ConnectorHealth {
        crate::north::ConnectorHealth::default()
    }
    fn is_enabled(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn test_north_dispatcher_fanout() {
    let (tx, rx) = crate::north::NorthFeed::channel(16);
    let mgr = NorthDispatcher::new(rx);

    let s1 = Arc::new(TestConnector {
        name_str: "s1".into(),
        count: AtomicUsize::new(0),
        fail: false,
    });
    let s2 = Arc::new(TestConnector {
        name_str: "s2".into(),
        count: AtomicUsize::new(0),
        fail: false,
    });
    mgr.register_connector(s1.clone() as Arc<dyn NorthConnector>);
    mgr.register_connector(s2.clone() as Arc<dyn NorthConnector>);
    assert_eq!(mgr.connector_count(), 2);

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;

    // 推 5 批（直接投递到北向专用队列）
    for i in 0..5 {
        tx.send(Arc::new(vec![DataPoint::good(
            &format!("p{}", i),
            "d1",
            i as f64,
            i as f64,
            "u16",
        )]))
        .await
        .unwrap();
    }

    // 等 fan-out 跑完
    for _ in 0..100 {
        if s1.count.load(Ordering::Relaxed) == 5 && s2.count.load(Ordering::Relaxed) == 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    // 每个 connector 收到 5 批
    assert_eq!(s1.count.load(Ordering::Relaxed), 5);
    assert_eq!(s2.count.load(Ordering::Relaxed), 5);

    // 触发 shutdown，run 应该退出
    mgr.shutdown_token().shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), run_handle).await;
}

#[tokio::test]
async fn test_north_dispatcher_empty_connectors() {
    let (tx, rx) = crate::north::NorthFeed::channel(16);
    let mgr = NorthDispatcher::new(rx);
    assert_eq!(mgr.connector_count(), 0);

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;

    // 推 1 批（无 connector 注册，不应 panic）
    tx.send(Arc::new(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    mgr.shutdown_token().shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), run_handle).await;
}

#[tokio::test]
async fn test_connector_publish_batch_no_panic_on_empty() {
    // 验证 publish_batch(&[]) 不会 panic
    let connector = TestConnector {
        name_str: "test".into(),
        count: AtomicUsize::new(0),
        fail: false,
    };
    let _ = connector.publish_batch(&[]).await;
    // TestConnector.publish_batch always increments (regardless of batch size)
    assert_eq!(connector.count.load(Ordering::Relaxed), 1);
    // 关闭 unused 警告
    let _ = Quality::Good;
}

/// r66-SD1 契约：`shutdown_token().shutdown()` 后 NorthDispatcher 的 run
/// JoinHandle 必须在 5s 内 resolve。
///
/// graceful_shutdown 依赖此保证——若 JoinHandle 不 resolve（shutdown 信号丢失
/// / run 循环死锁），停机序会白等 5s 才推进，且 systemd SIGKILL 时 run task
/// 会被硬杀，发送端计数异常。此处锁死该行为。
#[tokio::test]
async fn r66_north_dispatcher_run_exits_after_shutdown() {
    let (_tx, rx) = crate::north::NorthFeed::channel(16);
    let mgr = NorthDispatcher::new(rx);

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;

    // 触发 shutdown
    mgr.shutdown_token().shutdown();
    let res = tokio::time::timeout(std::time::Duration::from_secs(5), run_handle).await;
    assert!(
        res.is_ok(),
        "r66: shutdown 后 NorthDispatcher run 循环必须退出（停机序 await 其 JoinHandle）"
    );
    // 句柄应 Ok(())——run 正常 break（非 panic 路径）
    assert!(
        res.unwrap().is_ok(),
        "r66: NorthDispatcher run 应正常结束（Ok），不应是 JoinError"
    );
}

/// 第 5 层验收：发送端全部 drop 后 run 必须自然退出（不依赖 shutdown 信号）。
#[tokio::test]
async fn north_dispatcher_exits_when_feed_closed() {
    let (tx, rx) = crate::north::NorthFeed::channel(4);
    let mgr = NorthDispatcher::new(rx);
    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;

    drop(tx);
    let res = tokio::time::timeout(std::time::Duration::from_secs(5), run_handle).await;
    assert!(res.is_ok(), "发送端关闭后 run 必须退出，否则停机序会白等");
    assert!(res.unwrap().is_ok(), "run 应正常结束");
}

/// 第 5 层验收：小容量 + 慢消费时 send 背压等待而非丢批，
/// 最终所有批次都到达 connector（对比广播总线会 Lagged 丢帧）。
#[tokio::test]
async fn north_feed_backpressure_delivers_every_batch() {
    let total = 32usize;
    let (tx, rx) = crate::north::NorthFeed::channel(1);
    let mgr = NorthDispatcher::new(rx);
    let connector = Arc::new(TestConnector {
        name_str: "bp".into(),
        count: AtomicUsize::new(0),
        fail: false,
    });
    mgr.register_connector(connector.clone() as Arc<dyn NorthConnector>);

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;

    for i in 0..total {
        tx.send(Arc::new(vec![DataPoint::good(
            &format!("p{}", i),
            "d1",
            i as f64,
            i as f64,
            "u16",
        )]))
        .await
        .expect("队列满时应等待而不是失败");
    }

    for _ in 0..200 {
        if connector.count.load(Ordering::Relaxed) == total {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(
        connector.count.load(Ordering::Relaxed),
        total,
        "背压期间不得丢批（广播总线下会被 Lagged 截断）"
    );
    assert!(
        tx.stats().backpressure_events > 0,
        "容量 1 + 连续投递应至少触发一次背压"
    );

    mgr.shutdown_token().shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), run_handle).await;
}

/// I-51：单个 connector 失败只记 warn，不得阻断其它 connector 收齐全部批次。
///
/// `fanout` 用 `join_all` 并发投递，各 connector 的 `publish_batch` 结果彼此独立；
/// 若退化成「一个失败就 return」，健康 connector 会少收批次。
#[tokio::test]
async fn failing_connector_does_not_block_others() {
    let total = 5usize;
    let (tx, rx) = crate::north::NorthFeed::channel(16);
    let mgr = NorthDispatcher::new(rx);

    let failing = Arc::new(TestConnector {
        name_str: "bad".into(),
        count: AtomicUsize::new(0),
        fail: true,
    });
    let healthy = Arc::new(TestConnector {
        name_str: "good".into(),
        count: AtomicUsize::new(0),
        fail: false,
    });
    mgr.register_connector(failing.clone() as Arc<dyn NorthConnector>);
    mgr.register_connector(healthy.clone() as Arc<dyn NorthConnector>);

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;

    for i in 0..total {
        tx.send(Arc::new(vec![DataPoint::good(
            &format!("p{}", i),
            "d1",
            i as f64,
            i as f64,
            "u16",
        )]))
        .await
        .unwrap();
    }

    for _ in 0..200 {
        if healthy.count.load(Ordering::Relaxed) == total {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    assert_eq!(
        healthy.count.load(Ordering::Relaxed),
        total,
        "失败 connector 不得阻断其它 connector 收齐全部批次"
    );
    assert_eq!(
        failing.count.load(Ordering::Relaxed),
        total,
        "失败 connector 每批仍会被调用（错误只在 fanout 内记 warn）"
    );

    mgr.shutdown_token().shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(500), run_handle).await;
}

/// I-51：停机信号到达时，队列中已积压的批次必须先 drain 再退出。
///
/// 停机窗口里 `run` 若直接 break，末尾几批上行数据会永久丢失；
/// 正确语义是 shutdown 分支先 `try_recv` 把积压全部 fan-out 出去。
#[tokio::test]
async fn dispatcher_drains_pending_batches_before_exit() {
    let total = 5usize;
    let (tx, rx) = crate::north::NorthFeed::channel(16);
    let mgr = NorthDispatcher::new(rx);

    let connector = Arc::new(TestConnector {
        name_str: "drain".into(),
        count: AtomicUsize::new(0),
        fail: false,
    });
    mgr.register_connector(connector.clone() as Arc<dyn NorthConnector>);

    // 先在队列里积压，run 尚未启动（接收端还没被 take 走）
    for i in 0..total {
        tx.send(Arc::new(vec![DataPoint::good(
            &format!("p{}", i),
            "d1",
            i as f64,
            i as f64,
            "u16",
        )]))
        .await
        .unwrap();
    }

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    // 立刻停机：此时队列仍有积压
    mgr.shutdown_token().shutdown();

    let res = tokio::time::timeout(std::time::Duration::from_secs(5), run_handle)
        .await
        .expect("停机后 run 必须在 5s 内退出");
    assert!(res.is_ok(), "run 应正常结束");

    assert_eq!(
        connector.count.load(Ordering::Relaxed),
        total,
        "停机前已积压的批次必须全部 drain，不得被停机窗口截断"
    );
}
