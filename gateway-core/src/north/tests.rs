use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 测试用 connector：记录收到的点数
struct CountConnector {
    count: AtomicUsize,
}

#[async_trait::async_trait]
impl NorthConnector for CountConnector {
    fn name(&self) -> &str {
        "test_count"
    }
    async fn start(&self) -> GatewayResult<()> {
        Ok(())
    }
    async fn stop(&self) {}
    async fn publish_batch(&self, batch: &[DataPoint]) -> GatewayResult<()> {
        self.count.fetch_add(batch.len(), Ordering::Relaxed);
        Ok(())
    }
    fn health(&self) -> ConnectorHealth {
        ConnectorHealth::default()
    }
    fn is_enabled(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn test_connector_register_and_publish() {
    let connector = Arc::new(CountConnector {
        count: AtomicUsize::new(0),
    });
    let points = vec![
        DataPoint::good("p1", "d1", 1.0, 1.0, "u16"),
        DataPoint::good("p2", "d1", 2.0, 2.0, "u16"),
    ];
    connector.publish_batch(&points).await.unwrap();
    assert_eq!(connector.count.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn test_connector_publish_multiple_batches() {
    let connector = Arc::new(CountConnector {
        count: AtomicUsize::new(0),
    });
    for _ in 0..5 {
        connector
            .publish_batch(&[DataPoint::good("p", "d", 1.0, 1.0, "u16")])
            .await
            .unwrap();
    }
    assert_eq!(connector.count.load(Ordering::Relaxed), 5);
}

#[test]
fn test_connector_name() {
    let connector = CountConnector {
        count: AtomicUsize::new(0),
    };
    assert_eq!(connector.name(), "test_count");
}

#[tokio::test]
async fn test_connector_async_fanout_concept() {
    // 验证 publish_batch trait 设计契约：
    // - 接受 &[DataPoint]（借用引用）
    // - publish_batch 为 async + 非阻塞语义
    // - 同 connector 实例多次调用累加计数
    let connector = Arc::new(CountConnector {
        count: AtomicUsize::new(0),
    });
    let mut handles = Vec::new();
    for _ in 0..10 {
        let c = connector.clone();
        handles.push(tokio::task::spawn(async move {
            c.publish_batch(&[DataPoint::good("p", "d", 1.0, 1.0, "u16")])
                .await
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(connector.count.load(Ordering::Relaxed), 10);
}
