//! Gauge 类指标操作（可增可减的仪表盘）

use super::Metrics;
use std::sync::atomic::Ordering;

impl Metrics {
    /// 设置队列深度
    pub fn set_queue_depth(&self, depth: i64) {
        self.queue_depth.store(depth, Ordering::Relaxed);
    }

    /// 设置在线/离线点位数
    pub fn set_point_status(&self, online: i64, offline: i64) {
        self.points_online.store(online, Ordering::Relaxed);
        self.points_offline.store(offline, Ordering::Relaxed);
    }

    /// S-5: 设置 MQTT in-flight 数量
    pub fn set_in_flight(&self, n: i64) {
        self.mqtt_in_flight.store(n, Ordering::Relaxed);
    }

    pub fn set_outbox_pending(&self, n: i64) {
        self.outbox_pending.store(n, Ordering::Relaxed);
    }

    /// r52-MEM1：设置进程 RSS 字节数。传入 0 表示采集失败（无副作用，便于上层判别）。
    pub fn set_process_rss(&self, bytes: i64) {
        self.process_rss_bytes.store(bytes, Ordering::Relaxed);
    }

    /// 设置内置模拟消费者的运行状态（1 = 运行中，0 = 未运行）。
    pub fn set_sim_consumer_running(&self, running: bool) {
        self.sim_consumer
            .running
            .store(if running { 1 } else { 0 }, Ordering::Relaxed);
    }

    /// P9：设置磁盘压力标志（1 = 剩余空间低于阈值、入队被阻断；0 = 正常）。
    ///
    /// 磁盘满时网关会阻断入队、低优先级点被丢弃——这是真实的生产影响，
    /// 此前没有任何指标暴露，运维只能从"点没上来"反推。
    pub fn set_disk_pressure(&self, pressure: i64) {
        self.disk_pressure.store(pressure, Ordering::Relaxed);
    }

    /// 设置磁盘水位档位（0 = ok / 1 = warn / 2 = critical / 3 = blocking）。
    ///
    /// 二值的 `gateway_disk_pressure` 只能在"已阻断"那一刻给出信号，
    /// 现场无法提前判断"离写满还有多远"；分级 gauge 让 Prometheus 能对
    /// `warn` / `critical` 档直接配告警规则。
    pub fn set_disk_level(&self, level: i64) {
        self.disk_level.store(level, Ordering::Relaxed);
    }
}
