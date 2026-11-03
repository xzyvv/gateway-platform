//! 合并参数自适应统计与写分组辅助
//!
//! 决策表 #3（Modbus 合并参数自适应成本模型）的运行时状态，
//! 以及批量写入路径用到的分组元素类型与反向换算。

use crate::config::PointConfig;

/// 合并参数自适应统计（EMA 移动平均）
///
/// 决策表 #3（Modbus 合并参数自适应成本模型）：
/// - `avg_batch_size`: 最近 N 个"每个 batch 平均含多少 point"的移动平均
/// - 读路径开头根据`avg_batch_size` 计算下一轮的 `gap_threshold`。
///   - avg ≥ 8 时加大 gap（点位密集，可激进合并）
///   - avg < 8 时收紧 gap（点位稀疏，不合并不划算）
/// - `max_count` 不调整（Modbus 协议 125 寄存器 / 2000 线圈已是上限）
#[derive(Debug, Clone)]
pub(super) struct MergeStats {
    /// 最近所有批次的平均点位数（初始 0.0 表示"用默认值"）
    avg_batch_size: f64,
    /// EMA 衰减系数：new_avg = old_avg * decay + new_meas * (1 - decay)
    decay: f64,
    /// 累计已观测批次计数（用于决定是否打日志 / 多少轮后稳定）
    samples: u64,
}

impl MergeStats {
    pub(super) fn new() -> Self {
        Self {
            avg_batch_size: 0.0,
            decay: 0.9,
            samples: 0,
        }
    }

    /// 根据当前均值计算下一轮 gap。
    /// avg == 0 时默认 5；avg ≥ 8 时 8；avg < 8 时 1..5 线性。
    pub(super) fn next_gap(&self) -> u16 {
        if self.avg_batch_size < 0.5 {
            return 5; // 数据不足，用历史默认
        }
        // ratio in [0.5, 1.5] 对应 gap in [3, 8]
        let ratio = (self.avg_batch_size / 8.0).clamp(0.5, 1.5);
        let gap = (5.0 * ratio).round() as u16;
        gap.clamp(1, 20)
    }

    /// 记录一次"成功完成"的 batch 结果
    pub(super) fn observe(&mut self, batch_size: f64) {
        if self.avg_batch_size < 0.5 {
            // 首轮初始化为本次观测值
            self.avg_batch_size = batch_size;
        } else {
            self.avg_batch_size =
                self.avg_batch_size * self.decay + batch_size * (1.0 - self.decay);
        }
        self.samples = self.samples.saturating_add(1);
    }
}

/// 写分组成员：(地址, 单元数, 入参下标)
pub(super) type WriteItem = (u16, u16, usize);

/// 反向换算：工程量 `value` -> 原始值 `raw`。
///
/// 与 `write` 中的换算保持一致（公式无法反推，沿用「不反推公式」的既有行为）。
pub(super) fn reverse_convert(point: &PointConfig, value: f64) -> f64 {
    if point.scale != 0.0 {
        (value - point.offset) / point.scale
    } else {
        value
    }
}
