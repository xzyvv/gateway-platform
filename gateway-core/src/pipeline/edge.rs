//! 边缘处理管道（§8.2）
//!
//! 采用**插件化管道**架构：每个数据处理步骤是一个 `PipelineNode`，
//! 按配置顺序串联执行。新增处理逻辑只需实现 trait 并加入管道，
//! 无需修改 `EdgeProcessor` 主体。
//!
//! 当前内置节点：
//! - `AggregationNode`：窗口聚合（last/avg/max/min/sum）
//! - `DeadbandNode`：死区过滤
//! - `ChangeNode`：变化上传
//!
//! 扩展示例：`FormulaNode`（公式计算）、`UnitNode`（单位换算）、
//! `OutlierNode`（异常值剔除）等。

use parking_lot::Mutex;
use std::collections::HashMap;

use crate::config::PointConfig;
use crate::engine::model::DataPoint;
use crate::engine::model::{AggFn, Quality};

/// 管道节点 trait
///
/// 每个节点接收一个数据点，返回处理后的结果：
/// - `Some(dp)`：继续传给下一个节点（或最终上传）
/// - `None`：被该节点过滤，管道终止
///
/// 节点应保持无状态或内部状态线程安全。
///
/// `flush_aggregated` / `flush_all` 用于节点维护的时间窗口状态
/// （如 `AggregationNode` 的窗口缓冲）。默认实现返回空 Vec，
/// 无窗口状态的节点（`DeadbandNode`、`ChangeNode` 等）无需重写。
pub trait PipelineNode: Send + Sync {
    /// 节点名称（用于日志/调试）
    fn name(&self) -> &'static str;

    /// 处理单个数据点
    fn process(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint>;

    /// 处理"刷新产物"数据点（`flush_aggregated` / `flush_all` 的返回值再次进入管道时）
    ///
    /// 默认与 [`process`](Self::process) 相同。维护窗口缓冲的节点（`AggregationNode`）
    /// **必须**重写为直接放行：flush 产物已是最终结果，若再走 `process` 会被重新塞回
    /// 窗口缓冲并返回 `None`（该值当轮丢失），而下一轮 flush 又把它输出、再次重入 ——
    /// 形成"每秒重复上报同一个聚合值"的循环。
    fn process_flushed(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        self.process(point, dp)
    }

    /// 处理「质量恢复」样本（Bad/Uncertain → Good 的第一个 Good 样本）
    ///
    /// 必须放行：Bad 样本绕过管道，死区/变化节点并不知道发生过故障，其基线仍是
    /// 故障前的值；若恢复后的数值恰好相同，会被当作「未变化」过滤掉，云端将一直
    /// 停留在 Bad 状态。过滤类节点应重写为**更新基线并放行**；聚合节点保持默认
    /// 实现（恢复样本仍需进入窗口，不能漏统计）。
    fn process_recovered(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        self.process(point, dp)
    }

    /// 刷新所有到期的窗口状态（默认无操作）
    ///
    /// 周期性调用（如每秒），节点应返回已到期但尚未输出的数据点。
    /// 无窗口状态的节点返回空 Vec。
    fn flush_aggregated(&self, _points: &[PointConfig]) -> Vec<DataPoint> {
        Vec::new()
    }

    /// 强制刷新所有窗口状态（默认无操作）
    ///
    /// 用于退出前清理，节点应返回所有未输出的缓冲数据。
    fn flush_all(&self, _points: &[PointConfig]) -> Vec<DataPoint> {
        Vec::new()
    }
}

/// 聚合缓冲区
use super::nodes::*;

pub struct EdgeProcessor {
    nodes: Vec<Box<dyn PipelineNode>>,
    /// 每个点位上一次样本的质量，用于识别 Bad/Uncertain → Good 的恢复边沿
    last_quality: Mutex<HashMap<String, Quality>>,
}

impl EdgeProcessor {
    /// 创建默认管道：聚合 → 死区 → 变化上传
    ///
    /// `AggregationNode` 位于首位，对 Good 样本进入聚合缓冲区；
    /// 非聚合点位会原样透传给后续节点。
    pub fn new() -> Self {
        let nodes: Vec<Box<dyn PipelineNode>> = vec![
            Box::new(AggregationNode::new()),
            Box::new(DeadbandNode::new()),
            Box::new(ChangeNode::new()),
        ];
        Self {
            nodes,
            last_quality: Mutex::new(HashMap::new()),
        }
    }

    /// 用自定义节点列表创建管道
    ///
    /// 调用方完全控制节点序列，不强制注入 `AggregationNode`。
    /// 若未包含聚合节点，`flush_aggregated` / `flush_all` 将返回空 Vec。
    pub fn with_nodes(nodes: Vec<Box<dyn PipelineNode>>) -> Self {
        Self {
            nodes,
            last_quality: Mutex::new(HashMap::new()),
        }
    }

    /// 处理一个采集数据点
    ///
    /// 依次经过管道中的所有节点。返回 `Some(dp)` 表示需要上传，
    /// `None` 表示被过滤（或已进入聚合缓冲区）。
    ///
    /// 采集失败的数据始终上传（让平台感知设备状态），跳过所有节点。
    pub fn process(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        // 质量边沿：Bad/Uncertain → Good 的恢复样本必须上行一次（见
        // `PipelineNode::process_recovered`）。质量更新放在最前面，Bad 样本
        // 也要记账，否则恢复边沿永远识别不到。
        let recovered = {
            let mut last_quality = self.last_quality.lock();
            let prev = last_quality.insert(dp.point_id.clone(), dp.quality);
            prev.is_some_and(|q| q != Quality::Good) && dp.quality == Quality::Good
        };
        if dp.quality != Quality::Good {
            return Some(dp);
        }
        let mut current = dp;
        if recovered {
            for node in &self.nodes {
                current = node.process_recovered(point, current)?;
            }
            return Some(current);
        }
        for node in &self.nodes {
            current = node.process(point, current)?;
        }
        Some(current)
    }

    /// 处理"刷新产物"数据点（聚合窗口 flush 的返回值）
    ///
    /// 与 [`process`](Self::process) 的唯一区别是逐节点调用
    /// [`PipelineNode::process_flushed`](PipelineNode::process_flushed) 而非 `process`：
    /// 聚合节点在该路径下直接放行，避免把已完成的聚合值重新塞回窗口缓冲。
    /// 其余节点（死区/变化上传）默认实现等同 `process`，行为不变。
    pub fn process_flushed(&self, point: &PointConfig, dp: DataPoint) -> Option<DataPoint> {
        if dp.quality != Quality::Good {
            return Some(dp);
        }
        let mut current = dp;
        for node in &self.nodes {
            current = node.process_flushed(point, current)?;
        }
        Some(current)
    }

    /// 刷新所有到期的窗口状态
    ///
    /// 遍历管道中所有节点，收集各自到期的聚合数据点。
    pub fn flush_aggregated(&self, points: &[PointConfig]) -> Vec<DataPoint> {
        self.nodes
            .iter()
            .flat_map(|n| n.flush_aggregated(points))
            .collect()
    }

    /// 强制刷新所有窗口状态（用于退出前）
    pub fn flush_all(&self, points: &[PointConfig]) -> Vec<DataPoint> {
        self.nodes
            .iter()
            .flat_map(|n| n.flush_all(points))
            .collect()
    }
}

impl Default for EdgeProcessor {
    fn default() -> Self {
        Self::new()
    }
}

/// 聚合计算
pub(crate) fn aggregate(values: &[(i64, f64)], func: AggFn) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    match func {
        AggFn::Last => values.last().map(|(_, v)| *v).unwrap_or(0.0),
        AggFn::Avg => {
            let sum: f64 = values.iter().map(|(_, v)| v).sum();
            sum / values.len() as f64
        }
        AggFn::Max => values
            .iter()
            .map(|(_, v)| *v)
            .fold(f64::NEG_INFINITY, f64::max),
        AggFn::Min => values.iter().map(|(_, v)| *v).fold(f64::INFINITY, f64::min),
        AggFn::Sum => values.iter().map(|(_, v)| v).sum(),
    }
}

#[cfg(test)]
mod tests;
