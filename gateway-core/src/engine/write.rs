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
}
