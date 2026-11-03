use super::*;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 测试用 connector：只累加计数
struct TestConnector {
    name_str: String,
    count: AtomicUsize,
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
async fn test_conversion_manager_fanout() {
    let bus = RealtimeBus::new(16);
    let mgr = ConversionManager::new(bus.clone());

    let s1 = Arc::new(TestConnector {
        name_str: "s1".into(),
        count: AtomicUsize::new(0),
    });
    let s2 = Arc::new(TestConnector {
        name_str: "s2".into(),
        count: AtomicUsize::new(0),
    });
    mgr.register_connector(s1.clone() as Arc<dyn NorthConnector>);
    mgr.register_connector(s2.clone() as Arc<dyn NorthConnector>);
    assert_eq!(mgr.connector_count(), 2);

    // 启动 run 在后台 task（broadcast 必须在 publish 前有 receiver，否则消息丢失）
    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    // 等 ConversionManager 内部 subscribe 完成（spawn 后 yield 一次）
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // 推 5 批（RealtimeBus.publish 接收 Arc<Vec<DataPoint>>）
    for i in 0..5 {
        bus.publish(Arc::new(vec![DataPoint::good(
            &format!("p{}", i),
            "d1",
            i as f64,
            i as f64,
            "u16",
        )]));
    }

    // 等 fan-out 跑完
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // 每个 connector 收到 5 批
    assert_eq!(s1.count.load(Ordering::Relaxed), 5);
    assert_eq!(s2.count.load(Ordering::Relaxed), 5);

    // 触发 shutdown，run 应该退出
    mgr.shutdown_token().shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_millis(200), run_handle).await;
}

#[tokio::test]
async fn test_conversion_manager_empty_connectors() {
    let bus = RealtimeBus::new(16);
    let mgr = ConversionManager::new(bus.clone());
    assert_eq!(mgr.connector_count(), 0);

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // 推 1 批（无 connector 注册，不应 panic）
    bus.publish(Arc::new(vec![DataPoint::good("p", "d", 1.0, 1.0, "u16")]));
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
    };
    let _ = connector.publish_batch(&[]).await;
    // TestConnector.publish_batch always increments (regardless of batch size)
    assert_eq!(connector.count.load(Ordering::Relaxed), 1);
    // 关闭 unused 警告
    let _ = Quality::Good;
}

/// r66-SD1 契约：`shutdown_token().shutdown()` 后 ConversionManager 的 run
/// JoinHandle 必须在 5s 内 resolve。
///
/// graceful_shutdown 依赖此保证——若 JoinHandle 不 resolve（shutdown 信号丢失
/// / run 循环死锁），停机序会白等 5s 才推进，且 systemd SIGKILL 时 run task
/// 会被硬杀，RealtimeBus sender 计数异常。此处锁死该行为。
#[tokio::test]
async fn r66_conversion_manager_run_exits_after_shutdown() {
    let bus = RealtimeBus::new(16);
    let mgr = ConversionManager::new(bus.clone());

    let mgr_clone = mgr.clone();
    let run_handle = tokio::spawn(async move { mgr_clone.run().await });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // 触发 shutdown
    mgr.shutdown_token().shutdown();
    let res = tokio::time::timeout(std::time::Duration::from_secs(5), run_handle).await;
    assert!(
        res.is_ok(),
        "r66: shutdown 后 ConversionManager run 循环必须退出（停机序 await 其 JoinHandle）"
    );
    // 句柄应 Ok(())——run 正常 break（非 panic 路径）
    assert!(
        res.unwrap().is_ok(),
        "r66: ConversionManager run 应正常结束（Ok），不应是 JoinError"
    );
}
