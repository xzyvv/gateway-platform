//! 点位热重载：全量替换与增量更新
//!
//! 从 scheduler/mod.rs 拆分。

use std::collections::HashMap;
use std::sync::Arc;

use std::time::Duration;

use crate::config::PointConfig;

use super::{group_key_for_point, point_config_equal, Scheduler};

impl Scheduler {
    /// Bug-R5 (fix)：返回 `Result<(), String>` 让上层 reload() 可在异常时回滚。
    ///
    /// 旧 `()` 返回类型隐藏错误——若内部 spawn panic（被 supervision 吸收但不传
    /// 出）或 bus task join 超时，外部 reload() 收到"成功"继续走 device_states 清理，
    /// 而 collectors / points 已被部分替换 → 数据采集路由处于不一致状态。
    pub(crate) async fn reload_points_incremental(
        self: &Arc<Self>,
        new_points: Vec<PointConfig>,
        force_buses: &std::collections::HashSet<String>,
    ) -> Result<(), String> {
        // Bug r09-RLO1 (fix): 入口加 Semaphore 串行化——防止并发 reload 互相覆盖。
        // acquire_owned 在 permit drop 时自动释放，await 期间不阻塞后续 reload 排队。
        let _permit = self.reload_lock.clone().acquire_owned().await;

        // Bug-R5 (fix) 测试夹具入口：单测可通过 `Scheduler::fail_reload_injector`
        // 强制让 reload_points_incremental 返回 Err，验证 reload() 的回滚路径。
        // 注入器是**实例字段**（非全局 static），并行测试互不影响；
        // 生产构建（cfg(test) 不编译）下该字段不存在。
        #[cfg(test)]
        if let Some(err) = self.fail_reload_injector.lock().clone() {
            tracing::warn!("Bug-R5 test fail-injector active, returning Err({})", err);
            return Err(err);
        }

        let started_at = std::time::Instant::now();
        let old_points = self.points.lock().clone();
        tracing::info!(
            "reload_points_incremental: {} -> {} points",
            old_points.len(),
            new_points.len()
        );

        // 1. 按组键分组新点位（与 start() 分组逻辑一致）
        let mut new_groups: HashMap<String, (String, Vec<PointConfig>)> = HashMap::new();
        let mut by_bus: HashMap<String, Vec<PointConfig>> = HashMap::new();
        for p in &new_points {
            by_bus.entry(p.bus.clone()).or_default().push(p.clone());
        }
        for (bus_name, bus_points) in &by_bus {
            // 统一按 protocol 分组，GPIO 不再特殊处理
            let mut by_proto: HashMap<String, Vec<PointConfig>> = HashMap::new();
            for p in bus_points {
                by_proto
                    .entry(p.protocol.clone())
                    .or_default()
                    .push(p.clone());
            }
            for (proto, pts) in by_proto {
                // #13: 与 spawn_collector_task 共用同一 group_key，避免口径漂移
                let key = super::group_key(&proto, bus_name);
                new_groups.insert(key, (bus_name.clone(), pts));
            }
        }

        // 2. 对比旧组（从当前 handles 键 + 旧 points 推断），决定哪些需要重建
        let old_keys: std::collections::HashSet<String> =
            self.handles.lock().keys().cloned().collect();
        let new_keys: std::collections::HashSet<String> = new_groups.keys().cloned().collect();

        // 需要重建的组：新增的 或 点位内容变化的
        let mut to_rebuild: Vec<(String, String, Vec<PointConfig>)> = Vec::new();
        for (key, (bus_name, pts)) in &new_groups {
            if !old_keys.contains(key) {
                to_rebuild.push((key.clone(), bus_name.clone(), pts.clone()));
            } else {
                // 点位内容是否变化（按 id 集合 + 关键字段对比）
                let old_in_group: Vec<&PointConfig> = old_points
                    .iter()
                    .filter(|p| group_key_for_point(p) == *key)
                    .collect();
                // 点位内容变化 或 总线被强制重建（collector/串口更换）
                let changed = old_in_group.len() != pts.len()
                    || force_buses.contains(bus_name)
                    || old_in_group.iter().any(|op| {
                        !pts.iter()
                            .any(|np| np.id == op.id && point_config_equal(op, np))
                    });
                if changed {
                    to_rebuild.push((key.clone(), bus_name.clone(), pts.clone()));
                }
            }
        }

        // 需要停止的组：旧组中不在新组里的
        // Bug C7: handles 里除点位分组键外还有内部任务键（如 "_aggregation_flush"），
        // 它们不在 new_keys 中，若一并 stop 会让聚合刷新任务被永久停掉
        // （reload 只重建点位分组、不会重建它）。此处显式排除内部键。
        let to_stop: Vec<String> = old_keys
            .difference(&new_keys)
            .filter(|k| k.as_str() != "_aggregation_flush")
            .cloned()
            .collect();

        if to_rebuild.is_empty() && to_stop.is_empty() {
            self.metrics.inc_config_reload();
            tracing::info!(
                "reload_points_incremental: no group changes, skipping ({} ms)",
                started_at.elapsed().as_millis()
            );
            *self.points.lock() = new_points;
            return Ok(());
        }

        tracing::info!(
            "reload_points_incremental: rebuild {} groups, stop {} groups",
            to_rebuild.len(),
            to_stop.len()
        );

        // 3. 停止需要重建和移除的组
        //    注意：不能在持有 MutexGuard 时 await（MutexGuard 不是 Send），
        //    否则整个 reload 链的 future 会变成 !Send，导致 web handler 无法注册。
        let shutdown_timeout = Duration::from_secs(3);
        let mut to_shutdown: Vec<(String, tokio::task::JoinHandle<()>)> = Vec::new();
        {
            let mut handles = self.handles.lock();
            for key in to_stop.iter().chain(to_rebuild.iter().map(|(k, _, _)| k)) {
                if let Some(h) = handles.remove(key) {
                    to_shutdown.push((key.clone(), h));
                }
            }
        } // MutexGuard 在此释放
          // 修复(审查#B1)：先逐组发送**本组独立**停止信号（取出 sender 即解除登记），
          // 让目标组在当前 tick 结束后优雅退出。原实现只能依赖全局 shutdown，而全局
          // 信号会连带停掉所有组，于是每组都只能空等满 3s 超时后被 abort。
        for (key, _) in &to_shutdown {
            if let Some(tx) = self.group_stops.lock().remove(key) {
                let _ = tx.send(true);
            }
        }
        // 修复(审查#B1)：并发等待全部目标组退出并共享同一次总超时预算。
        // 原实现逐组串行 `timeout(3s, &mut h)`，20 组将阻塞 API 达 60s。
        let joins = to_shutdown.into_iter().map(|(key, mut h)| async move {
            match tokio::time::timeout(shutdown_timeout, &mut h).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("group {} task panicked: {}", key, e),
                Err(_) => {
                    tracing::warn!("group {} task did not exit, aborting", key);
                    h.abort();
                }
            }
        });
        futures::future::join_all(joins).await;

        // 4. 更新 points
        *self.points.lock() = new_points;

        // 5. 清理已移除点位的 device_states，并按新配置刷新采集间隔
        let new_ids: std::collections::HashSet<String> =
            self.points.lock().iter().map(|p| p.id.clone()).collect();
        // 修复(审查#A1)：DeviceState 只在**首次采集**时按 point.interval 创建
        // （results.rs 的 `or_insert_with`），而本函数用 `retain` 保留旧条目 →
        // 改完配置里的 interval 触发热重载后，组虽然被重建，interval_fn 仍读到
        // 旧的 current_interval（spawning.rs 只读 current_interval），采集间隔
        // 永远不生效——现场表现为「改了配置但采集频率没变」。
        // 此处先取值再锁 states，避免 states→points 反向加锁造成死锁。
        let new_intervals: HashMap<String, Duration> = self
            .points
            .lock()
            .iter()
            .map(|p| (p.id.clone(), Duration::from_secs(p.interval.max(1))))
            .collect();
        {
            let mut states = self.device_states.lock();
            states.retain(|id, _| new_ids.contains(id));
            for (id, base) in &new_intervals {
                if let Some(s) = states.get_mut(id) {
                    if s.base_interval != *base {
                        s.base_interval = *base;
                        // 未处于退避时立即生效；退避中的条目在下次 record_success /
                        // record_failure 时会基于新 base_interval 重算 current_interval
                        if s.consecutive_failures == 0 {
                            s.current_interval = *base;
                        }
                    }
                }
            }
        }

        // 6. 重建变更组
        self.metrics.inc_config_reload();
        tracing::info!(
            "reload_points_incremental: rebuilding {} groups, stopped {} groups ({} ms into reload)",
            to_rebuild.len(),
            to_stop.len(),
            started_at.elapsed().as_millis()
        );
        for (key, bus_name, pts) in to_rebuild {
            // 所有协议（含 GPIO）统一走 spawn_collector_task（内部用同一 group_key）
            let _ = key; // key 仅用于日志/调试；spawn 内部按 group_key(protocol,bus) 生成同一键
            self.spawn_collector_task(&bus_name, pts);
        }
        Ok(())
    }
}
