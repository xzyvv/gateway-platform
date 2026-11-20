//! 写结果三态：跨协议统一的写语义口径
//!
//! 判据只有一个——**能否证明写没有生效**：
//!
//! | 状态 | 含义 | 上游该怎么办 |
//! |---|---|---|
//! | [`WriteOutcome::Ok`] | 写成功 | 正常后续 |
//! | [`WriteOutcome::Failed`] | 能证明写没有生效（设备拒绝 / 总线忙 / 参数非法…） | **可安全重试** |
//! | [`WriteOutcome::Uncertain`] | 证明不了（写帧已下发但没有可确认的应答） | **禁止自动重试**，先读回核实 |
//!
//! 为什么必须有第三态：写帧一旦下发就可能已经生效，重复下发会造成累加类点位
//! 翻倍、脉冲类点位多触发。「没收到应答」既不是成功也不是失败，把它混进
//! 普通失败里，上游看到「失败」就会重试——这正是现场「点位值莫名其妙翻倍」
//! 的成因之一。
//!
//! 分层：`engine` 是中立层，南向协议（`south`）只负责把各自的错误映射过来，
//! API 层只读这三态，不需要知道任何南向协议类型。

/// 单条写操作的结果三态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// 写成功：设备确认写已生效。
    Ok,
    /// 明确失败：能证明写**没有**生效，调用方可安全重试。
    Failed,
    /// 结果未知：写帧可能已下发并生效，但无法确认；**禁止**自动重试。
    Uncertain,
}

impl WriteOutcome {
    /// 契约层字符串形态（API 响应体 / 前端展示）。
    pub fn as_str(&self) -> &'static str {
        match self {
            WriteOutcome::Ok => "ok",
            WriteOutcome::Failed => "failed",
            WriteOutcome::Uncertain => "uncertain",
        }
    }

    /// 是否为「写很可能已执行」的结果未知状态（禁止自动重试）。
    pub fn is_uncertain(&self) -> bool {
        matches!(self, WriteOutcome::Uncertain)
    }
}

/// 写结果未知时「读回核实」的结论（G-03）。
///
/// 只做**正向收敛**：读回值与写入值一致才判定写已生效；不一致时保留
/// 「结果未知」，因为设备可能对写入值做限幅、联锁复位、或该值已被其他主站
/// 改写——读回不等**证明不了**写没生效（见不变量 W-03 / W-13）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReadbackVerdict {
    /// 读回值与写入值一致：写已生效，可把「结果未知」收敛为成功。
    Confirmed,
    /// 读回成功但值与写入值不一致：仍无法确认写入是否生效。
    Mismatch {
        /// 实际读回的值（工程量），随错误信息一并返回给操作员。
        readback: f64,
    },
}

/// 判断读回值是否可视为「写入已生效」。
///
/// 容差取**半个量化单位**：原始值的最小步长是 1，经 `scale` 换算后的工程值
/// 步长为 `|scale|`，四舍五入的正常误差不超过半个步长。例如 `scale = 0.1`
/// 时容差为 0.05，写入 220.0 而读回 220.0 判定一致。
///
/// 容差**刻意不用 `deadband`**：`deadband` 是「变化才上传」的过滤阈值，典型值
/// 远大于量化误差（如 deadband=1.0），拿来做读回比对会把 100 与 100.9 误判成
/// 一致，从而把没生效的写误报成成功。
///
/// 含 `formula` 的点位工程值可能是非线性换算，此时本判据偏保守：宁可判
/// 不一致（保持结果未知），也不误判成功。
pub fn write_readback_matches(written: f64, readback: f64, scale: f64) -> bool {
    if !written.is_finite() || !readback.is_finite() {
        return false;
    }
    let quant = scale.abs() * 0.5;
    let rel = written.abs() * 1e-9;
    let tol = quant.max(rel).max(1e-9);
    (readback - written).abs() <= tol
}

#[cfg(test)]
mod tests {
    use super::WriteOutcome;

    #[test]
    fn as_str_is_stable_contract() {
        assert_eq!(WriteOutcome::Ok.as_str(), "ok");
        assert_eq!(WriteOutcome::Failed.as_str(), "failed");
        assert_eq!(WriteOutcome::Uncertain.as_str(), "uncertain");
    }

    #[test]
    fn only_uncertain_is_uncertain() {
        assert!(WriteOutcome::Uncertain.is_uncertain());
        assert!(!WriteOutcome::Ok.is_uncertain());
        assert!(!WriteOutcome::Failed.is_uncertain());
    }

    // === 读回比对（G-03） ===

    #[test]
    fn readback_exact_match_is_confirmed() {
        assert!(super::write_readback_matches(220.0, 220.0, 0.1));
        assert!(super::write_readback_matches(0.0, 0.0, 1.0));
        assert!(super::write_readback_matches(-5.0, -5.0, 1.0));
    }

    #[test]
    fn readback_within_half_quantum_is_confirmed() {
        // scale=0.1 → 容差 0.05：量化误差以内算一致
        assert!(super::write_readback_matches(220.0, 220.04, 0.1));
        // scale=1 → 容差 0.5：读回 100.4 视为一致（原始值已四舍五入到 100）
        assert!(super::write_readback_matches(100.0, 100.4, 1.0));
    }

    #[test]
    fn readback_beyond_tolerance_is_mismatch() {
        // 差 0.06 > 0.05：判不一致（保持结果未知）
        assert!(!super::write_readback_matches(220.0, 220.06, 0.1));
        // 差 0.6 > 0.5：判不一致
        assert!(!super::write_readback_matches(100.0, 100.6, 1.0));
    }

    #[test]
    fn readback_deadband_is_not_used_as_tolerance() {
        // deadband 典型值 1.0；若误用它做容差，100 与 100.9 会被判一致。
        // 本判据只看 scale，因此必须判不一致。
        assert!(!super::write_readback_matches(100.0, 100.9, 0.1));
    }

    #[test]
    fn readback_non_finite_is_never_confirmed() {
        assert!(!super::write_readback_matches(f64::NAN, 1.0, 1.0));
        assert!(!super::write_readback_matches(1.0, f64::NAN, 1.0));
        assert!(!super::write_readback_matches(
            f64::INFINITY,
            f64::INFINITY,
            1.0
        ));
    }

    #[test]
    fn readback_zero_scale_falls_back_to_epsilon() {
        // scale=0 属于异常配置：容差不放大，只有精确相等才确认
        assert!(super::write_readback_matches(5.0, 5.0, 0.0));
        assert!(!super::write_readback_matches(5.0, 5.0001, 0.0));
    }
}
