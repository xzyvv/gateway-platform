//! sink 指标操作（async fan-out 相关）

use super::Metrics;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

impl Metrics {
    /// 记录一次 sink 投递（成功进入 mpsc）
    pub fn inc_sink_send(&self, name: &str) {
        self.fetch_add_to_map(&self.sink_send, name, 1);
    }

    /// 记录一次 sink 丢弃（mpsc 满）
    pub fn inc_sink_dropped(&self, name: &str) {
        self.fetch_add_to_map(&self.sink_dropped, name, 1);
    }

    /// 设置 sink 内部 mpsc 通道当前深度（由 sink task 定期上报）
    pub fn set_sink_channel_depth(&self, name: &str, depth: i64) {
        let mut map = self.sink_channel_depth.write();
        let entry = map.entry(name.to_string()).or_insert(AtomicI64::new(0));
        entry.store(depth.max(0), Ordering::Relaxed);
    }

    /// 读 sink send_total（snapshot）
    pub fn sink_send_total(&self, name: &str) -> u64 {
        self.sink_send
            .read()
            .get(name)
            .map(|a| a.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// 读 sink dropped_total（snapshot）
    pub fn sink_dropped_total(&self, name: &str) -> u64 {
        self.sink_dropped
            .read()
            .get(name)
            .map(|a| a.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// 读 sink 通道深度（snapshot）
    pub fn sink_channel_depth(&self, name: &str) -> i64 {
        self.sink_channel_depth
            .read()
            .get(name)
            .map(|a| a.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// 列出所有 sink name（用于 prometheus_export 遍历）
    pub fn sink_names(&self) -> Vec<String> {
        let send = self.sink_send.read();
        let mut names: Vec<String> = send.keys().cloned().collect();
        names.sort();
        names
    }

    /// 列出 dropped map 中所有 sink name（仅 dropped 无 send 的情况）
    pub fn sink_dropped_total_keys(&self) -> Vec<String> {
        let dropped = self.sink_dropped.read();
        let mut names: Vec<String> = dropped.keys().cloned().collect();
        names.sort();
        names
    }

    /// helper: 写路径上做 fetch_add，避免直接获取 AtomicU64 引用
    fn fetch_add_to_map(&self, map: &RwLock<HashMap<String, AtomicU64>>, name: &str, delta: u64) {
        let mut g = map.write();
        let entry = g.entry(name.to_string()).or_insert(AtomicU64::new(0));
        entry.fetch_add(delta, Ordering::Relaxed);
    }
}
