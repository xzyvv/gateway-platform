//! 公共直方图结构
//!
//! 取代旧 `Metrics` 上成组的 `*_sum / *_count / *_buckets` 三件套：
//! - 写入：`record(value_us, &bounds)` 一次完成 `sum += v; count += 1; bucket += 1`
//! - 导出：`render(name, &bounds, buf)` 输出 `bucket{le=...}` / `sum` / `count` 三组样本行
//! - 桶边界由调用方传入（两个用法的边界不同），本结构不硬编码
//!
//! 现有两个用法（Prom 名保持与拆分前完全一致）：
//! - `collect.duration`：`gateway_collect_duration_us_*`
//! - `transaction.duration`：`gateway_bus_transaction_duration_us_*`
//!
//! `render` 只输出样本行，`# HELP` / `# TYPE` 由调用方输出——这样可精确保留
//! 拆分前的 HELP 文案，避免 Grafana / 抓取端语义出现无谓变化。

use std::sync::atomic::{AtomicU64, Ordering};

/// 直方图桶数量（含末尾 `+Inf` 桶）。
pub const BUCKETS_LEN: usize = 10;

/// 通用直方图：累计和 + 样本数 + 分桶计数。
#[derive(Default)]
pub struct Histogram {
    sum: AtomicU64,
    count: AtomicU64,
    buckets: [AtomicU64; BUCKETS_LEN],
}

impl Histogram {
    /// 记录一次样本（`value_us` 为微秒）。
    ///
    /// 落入首个满足 `value_us <= bounds[i]` 的桶；超出全部边界则落入末桶。
    pub fn record(&self, value_us: u64, bounds: &[u64; BUCKETS_LEN]) {
        self.sum.fetch_add(value_us, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
        let idx = bounds
            .iter()
            .position(|&bound| value_us <= bound)
            .unwrap_or(BUCKETS_LEN - 1);
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// 输出样本行（累积语义：`le` 为该桶及以下的总计数）。
    ///
    /// `name` 为完整指标名（不含 `gateway_` 前缀），例如 `collect_duration_us`。
    pub fn render(&self, name: &str, bounds: &[u64; BUCKETS_LEN], buf: &mut String) {
        use std::fmt::Write as _;
        let mut cumulative = 0u64;
        for (idx, bound) in bounds.iter().enumerate() {
            cumulative = cumulative.saturating_add(self.buckets[idx].load(Ordering::Relaxed));
            let le = if idx == BUCKETS_LEN - 1 {
                "+Inf".to_string()
            } else {
                bound.to_string()
            };
            let _ = writeln!(buf, "gateway_{name}_bucket{{le=\"{le}\"}} {cumulative}");
        }
        let _ = writeln!(
            buf,
            "gateway_{name}_sum {}",
            self.sum.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            buf,
            "gateway_{name}_count {}",
            self.count.load(Ordering::Relaxed)
        );
    }

    /// 累计和（仅供测试断言）。
    #[cfg(test)]
    pub fn sum(&self) -> u64 {
        self.sum.load(Ordering::Relaxed)
    }

    /// 样本数（仅供测试断言）。
    #[cfg(test)]
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// 指定桶计数（仅供测试断言）。
    #[cfg(test)]
    pub fn bucket(&self, idx: usize) -> u64 {
        self.buckets[idx].load(Ordering::Relaxed)
    }
}
