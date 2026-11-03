//! Prometheus 文本格式导出（按子结构分段）
//!
//! 每个领域子结构自带 `render(&mut String)`，本文件只负责：
//! 1. 按固定顺序调用各子结构 render；
//! 2. 渲染留在 `Metrics` 顶层的 Gauge / 当日滚动计数器；
//! 3. 渲染 sink fan-out 三个标签化 map；
//! 4. 渲染前端兼容别名（uplink/queue_length/devices_online/errors_total）。
//!
//! 所有已存在于拆分前的指标名与 HELP 文案均保持不变。

use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use super::Metrics;

impl Metrics {
    /// 导出为 Prometheus 文本格式
    pub fn prometheus_export(&self) -> String {
        // r31-PM3: 容量从 1024 提升到 4096。典型导出含 30+ counter/gauge +
        // 2 个 histogram（10 桶）+ 若干 sink 标签行，1KB 偏小会触发 1-2 次 realloc。
        let mut out = String::with_capacity(4096);

        // ===== 领域子结构（各自负责 HELP/TYPE/样本行）=====
        self.mqtt.render(&mut out);
        self.collect.render(&mut out);
        self.webhook.render(&mut out);
        // 串口总线自检归因（设备级 vs 端口级）。C-2 的判定结论靠这几个计数
        // 才能在现场可见——只打 tracing 日志等于没有。
        self.bus.render(&mut out);
        self.alarm.render(&mut out);
        self.lifecyle.render(&mut out);
        // 队列持久化 / WAL / outbox 降级指标。此前该子结构的 render 从未被调用，
        // 导致 `gateway_queue_persist_*`、`gateway_wal_*`、`gateway_outbox_degraded_total`
        // 全部未导出（Prometheus 上不可见）。
        self.queue_persist.render(&mut out);

        // 处理管道 submit 成功率 / 丢失率（X1）。
        self.pipeline.render(&mut out);

        // 内置模拟消费者：消费条数 / 批次数 / 运行状态。
        self.sim_consumer.render(&mut out);

        // ===== 顶层 Gauge / 状态量 =====
        out.push_str("# HELP gateway_queue_depth Current data queue depth\n");
        out.push_str("# TYPE gateway_queue_depth gauge\n");
        out.push_str(&format!(
            "gateway_queue_depth {}\n",
            self.queue_depth.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP gateway_points_online Number of online points\n");
        out.push_str("# TYPE gateway_points_online gauge\n");
        out.push_str(&format!(
            "gateway_points_online {}\n",
            self.points_online.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP gateway_points_offline Number of offline points\n");
        out.push_str("# TYPE gateway_points_offline gauge\n");
        out.push_str(&format!(
            "gateway_points_offline {}\n",
            self.points_offline.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP gateway_outbox_pending Outbox pending messages\n");
        out.push_str("# TYPE gateway_outbox_pending gauge\n");
        out.push_str(&format!(
            "gateway_outbox_pending {}\n",
            self.outbox_pending.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP gateway_ota_progress OTA upgrade progress (0-100)\n");
        out.push_str("# TYPE gateway_ota_progress gauge\n");
        out.push_str(&format!(
            "gateway_ota_progress {}\n",
            self.ota_progress.load(Ordering::Relaxed)
        ));

        // P9：磁盘压力标志。1 = 剩余空间低于阈值，网关正在阻断入队、
        // 低优先级点被丢弃。此前的降采样标志只写不读，磁盘满完全不可见。
        out.push_str(
            "# HELP gateway_disk_pressure Disk pressure flag (1 = disk low, enqueue blocked)\n",
        );
        out.push_str("# TYPE gateway_disk_pressure gauge\n");
        out.push_str(&format!(
            "gateway_disk_pressure {}\n",
            self.disk_pressure.load(Ordering::Relaxed)
        ));

        out.push_str(
            "# HELP gateway_mqtt_in_flight MQTT in-flight records (dequeued, not acked)\n",
        );
        out.push_str("# TYPE gateway_mqtt_in_flight gauge\n");
        out.push_str(&format!(
            "gateway_mqtt_in_flight {}\n",
            self.mqtt_in_flight.load(Ordering::Relaxed)
        ));

        // 下行写入指标：成功/失败分开计数，供 Dashboard 展示真实写入错误数
        // （此前前端 write_error 恒 0，写失败在运行指标上不可见）。
        out.push_str("# HELP gateway_point_write_total Total point write attempts (single / batch / remote control / GPIO DO)\n");
        out.push_str("# TYPE gateway_point_write_total counter\n");
        out.push_str(&format!(
            "gateway_point_write_total {}\n",
            self.point_write_total.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP gateway_point_write_error_total Failed point write attempts\n");
        out.push_str("# TYPE gateway_point_write_error_total counter\n");
        out.push_str(&format!(
            "gateway_point_write_error_total {}\n",
            self.point_write_failures.load(Ordering::Relaxed)
        ));

        // r52-MEM1：进程 RSS（自监控）。0 表示采集失败/不支持平台。
        out.push_str("# HELP gateway_process_rss_bytes Process RSS in bytes (0 = unsupported)\n");
        out.push_str("# TYPE gateway_process_rss_bytes gauge\n");
        out.push_str(&format!(
            "gateway_process_rss_bytes {}\n",
            self.process_rss_bytes.load(Ordering::Relaxed)
        ));

        // ===== sink 指标（每个 sink 一组标签）=====
        out.push_str(
            "# HELP gateway_sink_send_total Successful sink.send() calls (entered mpsc)\n",
        );
        out.push_str("# TYPE gateway_sink_send_total counter\n");
        // r32-PM2: 一次性克隆 sink_send + sink_names 到本地，
        // 后续循环零锁竞争。sink_names() 内部还会 read 一次 sink_send 之外的表，
        // 故先建 send snapshot 再建 names。
        let send_snapshot: std::collections::HashMap<String, u64> = {
            let m = self.sink_send.read();
            m.iter()
                .map(|(k, a)| (k.clone(), a.load(Ordering::Relaxed)))
                .collect()
        };
        let names = self.sink_names();
        for name in &names {
            out.push_str(&format!(
                "gateway_sink_send_total{{sink=\"{}\"}} {}\n",
                name,
                send_snapshot.get(name).copied().unwrap_or(0)
            ));
        }

        out.push_str("# HELP gateway_sink_dropped_total Dropped batches due to mpsc full (async fan-out backpressure)\n");
        out.push_str("# TYPE gateway_sink_dropped_total counter\n");
        // r31-PM1 (修复)：原实现持锁期间克隆 keys + 后续 format 内又调用 snapshot
        // 再次获取 read 锁——同一函数 3 次加锁（names / dropped / depth），
        // metrics 写入侧（每个 sink task）都竞争同一 RwLock，Prometheus 抓取期间
        // 拖慢 metrics 写入。
        // 修复：一次性收集 3 个 map 的 keys（在最小锁粒度内），释放锁后再 format 输出。
        let mut all_names: BTreeSet<String> = names.iter().cloned().collect();
        {
            // 仅在 cloned collect 期间持有读锁——keys 克隆后立即释放
            let dropped_keys = self.sink_dropped.read();
            for k in dropped_keys.keys() {
                all_names.insert(k.clone());
            }
        }
        {
            let depth_keys = self.sink_channel_depth.read();
            for k in depth_keys.keys() {
                all_names.insert(k.clone());
            }
        }
        // r32-PM2: 把 dropped / depth 两个 map 也一次性克隆到本地 HashMap，
        // 后续循环查本地表——消除每个 sink name 一次的隐式 read 锁。
        // N 个 sink 节省 N×2 次锁竞争（N=已注册 sink 数；典型 3-5）。
        let dropped_snapshot: std::collections::HashMap<String, u64> = {
            let m = self.sink_dropped.read();
            m.iter()
                .map(|(k, a)| (k.clone(), a.load(Ordering::Relaxed)))
                .collect()
        };
        let depth_snapshot: std::collections::HashMap<String, i64> = {
            let m = self.sink_channel_depth.read();
            m.iter()
                .map(|(k, a)| (k.clone(), a.load(Ordering::Relaxed)))
                .collect()
        };
        for name in &all_names {
            out.push_str(&format!(
                "gateway_sink_dropped_total{{sink=\"{}\"}} {}\n",
                name,
                dropped_snapshot.get(name).copied().unwrap_or(0)
            ));
        }

        out.push_str("# HELP gateway_sink_channel_depth Current sink internal mpsc depth\n");
        out.push_str("# TYPE gateway_sink_channel_depth gauge\n");
        for name in &all_names {
            out.push_str(&format!(
                "gateway_sink_channel_depth{{sink=\"{}\"}} {}\n",
                name,
                depth_snapshot.get(name).copied().unwrap_or(0)
            ));
        }

        // 协议异常指标名内含协议名（如 `gateway_modbus_exception_*`），须置于 sink 段
        // 之后，以保证 sink 标签行在导出文本中先于任何协议名出现（r31-PM1 约定）。
        self.protocol_exception.render(&mut out);

        // ===== 单事务往返延迟（counter + histogram）=====
        self.transaction.render(&mut out);

        // ===== 按点位的采集失败分布（scoped 日桶）=====
        // 指标名不含协议名，不受 r31-PM1 的「协议名指标须在 sink 之后」约束，
        // 但仍放在 sink 段之后以保持导出文本的既有分区顺序。
        self.scoped.render(&mut out);

        // Frontend reads gateway_uplink_total / gateway_queue_length / gateway_devices_online / gateway_errors_total.
        // Exposed as aliases to avoid renaming existing metrics.
        let uplink_total = self.mqtt.publish_success.load(Ordering::Relaxed)
            + self.webhook.publish_success.load(Ordering::Relaxed);
        out.push_str(
            "# HELP gateway_uplink_total Total successful uplink messages (mqtt + webhook)\n",
        );
        out.push_str("# TYPE gateway_uplink_total counter\n");
        out.push_str(&format!("gateway_uplink_total {}\n", uplink_total));

        out.push_str(
            "# HELP gateway_queue_length Current data queue depth (alias of gateway_queue_depth)\n",
        );
        out.push_str("# TYPE gateway_queue_length gauge\n");
        out.push_str(&format!(
            "gateway_queue_length {}\n",
            self.queue_depth.load(Ordering::Relaxed)
        ));

        out.push_str("# HELP gateway_devices_online Number of online points (alias of gateway_points_online)\n");
        out.push_str("# TYPE gateway_devices_online gauge\n");
        out.push_str(&format!(
            "gateway_devices_online {}\n",
            self.points_online.load(Ordering::Relaxed)
        ));

        let errors_total = self.collect.serial_errors.load(Ordering::Relaxed)
            + self.collect.collect_failures.load(Ordering::Relaxed)
            + self.lifecyle.task_panics.load(Ordering::Relaxed);
        out.push_str("# HELP gateway_errors_total Total error count (serial + collect_failures + task_panics)\n");
        out.push_str("# TYPE gateway_errors_total counter\n");
        out.push_str(&format!("gateway_errors_total {}\n", errors_total));

        out
    }
}
