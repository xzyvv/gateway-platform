//! 协议注册表（`ProtocolRegistry`）：协议名 → 采集器工厂 / 帧提取器映射

use std::collections::HashMap;
use std::sync::Arc;

use super::entries::PROTOCOL_ENTRIES;
use super::{CollectorFactory, FrameExtractor};
use crate::engine::metrics::Metrics;
use crate::south::bus::BusManager;

use super::super::Collector;

/// 协议注册表
///
/// 维护协议名到采集器工厂、协议名到帧提取器的映射。
/// 注册内容由 [`PROTOCOL_ENTRIES`] 单一来源驱动，新增协议只需改 [`PROTOCOL_ENTRIES`]。
pub struct ProtocolRegistry {
    /// 协议名 -> Collector 工厂函数（运行时按需创建采集器实例）
    factories: HashMap<String, CollectorFactory>,
    /// 协议名 -> 帧提取器（仅串行总线协议注册；TCP/UDP 协议不需要）
    frame_extractors: HashMap<String, FrameExtractor>,
}

impl ProtocolRegistry {
    /// 创建注册表并注册内置协议（由 [`PROTOCOL_ENTRIES`] 驱动）
    pub fn new() -> Self {
        let mut registry = Self {
            factories: HashMap::new(),
            frame_extractors: HashMap::new(),
        };
        for entry in PROTOCOL_ENTRIES {
            registry
                .factories
                .insert(entry.name.to_string(), entry.factory);
            if let Some(fe) = entry.frame_extractor {
                registry.frame_extractors.insert(entry.name.to_string(), fe);
            }
        }
        registry
    }

    /// 注册一个协议的采集器工厂
    ///
    /// 保留 pub 接口供未来运行时扩展（如动态加载第三方协议）。内置协议已由
    /// [`PROTOCOL_ENTRIES`] 在 `new()` 中注册，通常无需手动调用。
    pub fn register(&mut self, protocol: &str, factory: CollectorFactory) {
        self.factories.insert(protocol.to_string(), factory);
    }

    /// 根据协议名创建采集器
    ///
    /// 返回 None 表示协议未注册：协议名拼错，或该协议被当前发行 feature（SKU）裁剪。
    /// 调用方应使用 [`crate::domain::protocol_error_message`] 生成可读错误，
    /// 不要把两种情况混为「网关不支持」。
    pub fn create(
        &self,
        protocol: &str,
        port_name: &str,
        bus: Arc<BusManager>,
        metrics: Arc<Metrics>,
    ) -> Option<Arc<dyn Collector>> {
        self.factories
            .get(protocol)
            .map(|factory| factory(port_name, bus, metrics))
    }

    /// 查询协议的帧提取器
    pub fn frame_extractor(&self, protocol: &str) -> Option<FrameExtractor> {
        self.frame_extractors.get(protocol).copied()
    }

    /// 列出所有已注册协议名
    pub fn protocols(&self) -> Vec<&str> {
        self.factories.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ProtocolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
