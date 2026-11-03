//! MqttClient 状态访问与变更（从 client.rs 拆出）
//!
//! 纯状态方法：配置查询、点位配置注入、模板编译缓存、断路器状态、连接状态。
//! 这些方法不直接触发 I/O，只读写 MqttClient 内部原子 / 锁字段。

use std::time::Duration;

use super::*;

impl MqttClient {
    /// 获取 MQTT 配置引用
    pub fn config(&self) -> &MqttConfig {
        &self.config
    }

    /// 注入点位配置（point_id → PointConfig），用于模板渲染时查找 point_name/unit/device_name 等元数据。
    /// 每次调用全量替换。可在 Web API 配置更新时调用。
    pub fn set_point_configs(&self, configs: Vec<crate::config::PointConfig>) {
        let mut map = self.point_configs.write();
        map.clear();
        for p in configs {
            map.insert(p.id.clone(), p);
        }
    }

    /// 获取编译后的自定义模板（若有）。
    ///
    /// 使用 OnceCell 做单次编译缓存：模板编译结果在 MqttClient 生命周期内不变。
    /// - 返回 `Some(&engine)` 表示模板已成功编译，调用方应使用模板渲染
    /// - 返回 `None` 表示两种情况之一：
    ///   a) `config.payload_template` 未配置或为空字符串（用户没启用自定义）
    ///   b) 模板编译失败（启动时记 error 日志，运行时静默降级为默认 JSON）
    ///   调用方不需区分这两种情况，统一走默认 JSON 格式即可。
    pub fn payload_template(&self) -> Option<&TemplateEngine> {
        // 优先检查源 config：空/未配置 → 直接返回 None（不进 OnceCell）
        let raw = self.config.payload_template.as_deref().unwrap_or("");
        if raw.trim().is_empty() {
            return None;
        }
        // 编译结果缓存到 OnceCell：仅在编译失败时记录错误日志，避免重复 panic
        let result = self.payload_template_engine.get_or_init(|| {
            match TemplateEngine::new(raw) {
                Ok(engine) => {
                    tracing::info!(
                        "mqtt: custom payload_template enabled ({} bytes)",
                        raw.len()
                    );
                    Ok(engine)
                }
                Err(e) => {
                    tracing::error!(
                        "mqtt: payload_template compile failed ({}), will fall back to default JSON format",
                        e
                    );
                    Err(e)
                }
            }
        });
        result.as_ref().ok()
    }

    /// 记录断路器失败：连续失败超过阈值则断开
    pub(crate) fn record_circuit_failure(&self) {
        let failures = self
            .circuit_failures
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        if failures >= CIRCUIT_FAILURE_THRESHOLD
            && !self.circuit_open.load(std::sync::atomic::Ordering::Relaxed)
        {
            self.circuit_open
                .store(true, std::sync::atomic::Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            self.circuit_opened_at
                .store(now, std::sync::atomic::Ordering::Relaxed);
            tracing::error!(
                "mqtt circuit breaker opened after {} consecutive failures, cooldown {}s",
                failures,
                CIRCUIT_COOLDOWN_SECS
            );
        }
    }

    /// C-7 修复：重置断路器失败计数（在 publish 成功后调用）。
    ///
    /// 旧实现仅在 `publish_batch` 成功路径重置失败计数，透传 `publish_bypass_queue`
    /// 成功路径不重置——意味着连续成功透传永远不能帮断路器"恢复"，因为
    /// 累计的失败计数不会因为成功而清零。
    /// 现在两条 publish 路径的成功分支都调本方法，行为一致。
    ///
    /// 注意：仅清失败计数，**不**强制关闭已开断路器——`circuit_open` 的关闭
    /// 仍由 `publish_batch` 的冷却期检查（`now - opened_at >= CIRCUIT_COOLDOWN_SECS`）
    /// 统一管理。半开 → 关闭的成功路径由 `publish_batch` 一处处理。
    pub(crate) fn reset_circuit_failures(&self) {
        self.circuit_failures
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// 断路器是否处于断开状态（供健康检查使用）
    pub fn is_circuit_open(&self) -> bool {
        self.circuit_open.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// MQTT 是否已连接
    pub fn is_connected(&self) -> bool {
        // Bug 54: connected 是 Arc<AtomicBool>，共享给事件循环
        self.connected.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 等待客户端连接成功（收到 ConnAck Success）。
    ///
    /// 用于热重载场景：在替换旧客户端前确认新客户端已连上 broker，
    /// 避免新客户端连不上时旧客户端已被停掉导致上报中断。
    ///
    /// 轮询间隔 100ms，超时返回 false。
    pub async fn wait_connected(&self, timeout: Duration) -> bool {
        if self.is_connected() {
            return true;
        }
        let start = std::time::Instant::now();
        loop {
            if self.is_connected() {
                return true;
            }
            if start.elapsed() >= timeout {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
