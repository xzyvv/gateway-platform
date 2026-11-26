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
//!
//! 本模块同时承载**跨入口统一写契约**（ROI-35）：请求/结果/拒绝原因、
//! 审计 detail 的统一格式与解析。执行逻辑在 `runtime::write_service`，
//! 因为只有运行态才拿得到 collector；`engine` 只定义中立数据形状。

use crate::engine::model::Quality;

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

/// 写请求的分派口径（与调用入口无关，单/批共用同一套执行逻辑）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    /// 单写：只允许一条；被拒（点位不存在 / 不可写 / 越界 / 无 collector）由
    /// 调用方映射成 HTTP 语义。
    Single,
    /// 批写：逐条独立成败，单条失败不影响其它条。
    Batch,
}

/// 一条写请求（点位 id + 目标值）。
#[derive(Debug, Clone)]
pub struct WriteRequest {
    pub id: String,
    pub value: f64,
}

/// 连下发都没有发生的拒绝原因分类。
///
/// 与「下发后失败」区分：拒绝原因可安全重试（修正配置/取值后），也必须在
/// 审计里留痕（DD-33：被拒写此前完全不留痕）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// 点位不在当前生效配置中
    NotFound,
    /// 点位未标记 `writable = true`
    NotWritable,
    /// 写入值超出 `write_min` / `write_max` 或不是有限数值
    OutOfRange,
    /// 点位所属总线没有对应 collector
    NoCollector,
}

impl RejectReason {
    /// 稳定的机器可读标签（审计 detail 的拒绝原因段 + 前端展示）。
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectReason::NotFound => "not_found",
            RejectReason::NotWritable => "not_writable",
            RejectReason::OutOfRange => "out_of_range",
            RejectReason::NoCollector => "no_collector",
        }
    }
}

/// 一次写操作的统一结果（单写 / 批写 / 北向写回共用）。
///
/// `outcome` 始终有效；`reject` 非空表示**未下发**（被拒），此时 `outcome`
/// 按「明确失败」处理。`message` 仅在失败/未知时存在。
#[derive(Debug, Clone)]
pub struct PointWriteResult {
    pub point_id: String,
    pub point_name: Option<String>,
    pub device_id: Option<String>,
    pub bus: String,
    pub value: f64,
    pub outcome: WriteOutcome,
    /// 失败/未知原因；成功为 `None`。
    pub message: Option<String>,
    /// 下发失败时映射出的质量码（被拒写为 `None`）。
    pub quality: Option<Quality>,
    /// 写结果未知且读回成功时的实际读回值（供操作员判断）。
    pub readback: Option<f64>,
    /// 非空表示连下发都没有发生（点位不存在 / 不可写 / 越界 / 无 collector）。
    pub reject: Option<RejectReason>,
}

impl PointWriteResult {
    /// 是否真正写成功（未被拒且设备确认）。
    pub fn is_ok(&self) -> bool {
        self.reject.is_none() && matches!(self.outcome, WriteOutcome::Ok)
    }

    /// 构造一条「未下发」的拒绝结果。
    pub fn rejected(
        point_id: impl Into<String>,
        point_name: Option<String>,
        device_id: Option<String>,
        bus: impl Into<String>,
        value: f64,
        reason: RejectReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            point_id: point_id.into(),
            point_name,
            device_id,
            bus: bus.into(),
            value,
            // 未下发 → 能证明写没有生效 → 明确失败，可安全重试
            outcome: WriteOutcome::Failed,
            message: Some(message.into()),
            quality: None,
            readback: None,
            reject: Some(reason),
        }
    }
}

/// 写调用的来源上下文（审计 source 的唯一来源）。
#[derive(Debug, Clone, Default)]
pub struct WriteContext {
    /// 通道标识：`api` / `opcua`。
    pub source: String,
    /// 真实操作人（前端上报 / 认证主体）；为空时回退到 `source`。
    pub operator: Option<String>,
    /// U-4：发起写入时的配置代次（跨 reload 写的追溯依据）。
    ///
    /// 允许跨代写（决议 A），但审计必须能回答「这次写发生在哪一代配置」。
    /// 北向（OPC UA）路径没有此值时为 `None`，审计不追加代次标记。
    pub config_generation: Option<u64>,
}

impl WriteContext {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            operator: None,
            config_generation: None,
        }
    }

    pub fn with_operator(mut self, operator: Option<String>) -> Self {
        self.operator = operator.filter(|s| !s.trim().is_empty());
        self
    }

    /// U-4：记录本次写入所属的配置代次。
    pub fn with_config_generation(mut self, generation: Option<u64>) -> Self {
        self.config_generation = generation;
        self
    }

    /// 审计记录里的 `source` 字段：**真实操作人优先**。
    ///
    /// 历史行为把来源写死成 `api` / `opcua`，现场只看到一串 `api`，无法回答
    /// 「是谁点的」（DD-36）。有操作人时直接记操作人，没有时退回通道名。
    pub fn audit_source(&self) -> &str {
        self.operator.as_deref().unwrap_or(&self.source)
    }
}

/// 写结果未知时的统一处置提示。
///
/// 提示文本同时用于逐条 `error` 与顶层业务码 message，让上游（前端 / MES /
/// 集成方）无论从哪个字段读到这次失败，都能得到同一个结论：
/// **先读回核实，不要自动重试**。
pub const WRITE_UNCERTAIN_HINT: &str = "设备可能已执行，请先读回核实，不要自动重试";

/// 按写结果三态生成面向用户的失败文案。
///
/// - `Uncertain`：追加 [`WRITE_UNCERTAIN_HINT`]，让调用方不会把它当成普通失败去重试；
/// - `Failed` / `Ok`：原样透传后端原因。
pub fn write_error_message(reason: &str, outcome: WriteOutcome) -> String {
    match outcome {
        WriteOutcome::Uncertain => format!("{reason}（{WRITE_UNCERTAIN_HINT}）"),
        WriteOutcome::Failed | WriteOutcome::Ok => reason.to_string(),
    }
}

/// 写审计 detail 的统一格式（控制历史与审计查询共用同一口径）。
///
/// - 成功：`{name}={value}`
/// - 明确失败：`{name}={value} [失败: {msg}]`
/// - 结果未知：`{name}={value} [超时: {msg}]`
///
/// `[超时:` 而不是 `[失败:` 是刻意的：结果未知与明确失败必须能分开，
/// 否则控制历史会把「可能已写入」显示成「失败」（DD-35）。
pub fn write_audit_detail(
    name: &str,
    value: f64,
    outcome: WriteOutcome,
    message: Option<&str>,
) -> String {
    match outcome {
        WriteOutcome::Ok => format!("{name}={value}"),
        WriteOutcome::Failed => format!("{name}={value} [失败: {}]", message.unwrap_or("写入失败")),
        WriteOutcome::Uncertain => format!(
            "{name}={value} [超时: {}]",
            message.unwrap_or("写结果未知（可能已写入，勿盲目重试）")
        ),
    }
}

/// 写审计 detail 的解析结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParsedWriteStatus {
    Ok,
    Failed,
    /// 结果未知（`[超时:` 前缀，或尾部值无法解析——**绝不能当成成功、值为 0**）
    Uncertain,
}

/// 解析后的写审计 detail。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedWriteDetail {
    /// 点位名（`=` 前的内容；名称可能本身含 `=`）
    pub name: String,
    /// 写入值；解析失败为 `None`（DD-35：不得回退成 0.0）
    pub value: Option<f64>,
    pub status: ParsedWriteStatus,
}

/// 去掉 U-4 追加的配置代次后缀 ` [cfg:<n>]`。
///
/// 代次只用于追溯，不参与写结果解析；不先剥离会让 `rsplit_once('=')`
/// 把值解析成 `12.5 [cfg:3` 而误判为未知。
fn strip_config_generation(detail: &str) -> &str {
    let trimmed = detail.trim_end();
    let Some(rest) = trimmed.strip_suffix(']') else {
        return trimmed;
    };
    let Some(pos) = rest.rfind(" [cfg:") else {
        return trimmed;
    };
    let gen = &rest[pos + " [cfg:".len()..];
    if !gen.is_empty() && gen.bytes().all(|b| b.is_ascii_digit()) {
        rest[..pos].trim_end()
    } else {
        trimmed
    }
}

/// 解析统一格式的写审计 detail。
///
/// 兼容历史记录：`[失败:` 视为明确失败，`[超时:` 视为结果未知。
/// 值解析失败时状态按「结果未知」处理并保留 `value = None`——把未知写显示成
/// 「成功、值 0」是最危险的误报（DD-35）。
pub fn parse_write_detail(detail: &str) -> ParsedWriteDetail {
    let detail = strip_config_generation(detail);
    let (body, status) = if let Some(pos) = detail.find(" [失败: ") {
        (&detail[..pos], ParsedWriteStatus::Failed)
    } else if let Some(pos) = detail.find(" [超时: ") {
        (&detail[..pos], ParsedWriteStatus::Uncertain)
    } else {
        (detail, ParsedWriteStatus::Ok)
    };
    let (name, value) = match body.rsplit_once('=') {
        Some((name, v)) => (name.to_string(), v.trim().parse::<f64>().ok()),
        None => (body.to_string(), None),
    };
    // 前缀说成功但值解析不出来：不能当作成功的 0，按未知处理
    let status = if status == ParsedWriteStatus::Ok && value.is_none() {
        ParsedWriteStatus::Uncertain
    } else {
        status
    };
    ParsedWriteDetail {
        name,
        value,
        status,
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

    // === 统一写审计 detail（ROI-35 / DD-35） ===

    #[test]
    fn audit_detail_format_is_tri_state() {
        use super::{write_audit_detail, WriteOutcome};
        assert_eq!(
            write_audit_detail("温度", 12.5, WriteOutcome::Ok, None),
            "温度=12.5"
        );
        assert_eq!(
            write_audit_detail("温度", 12.5, WriteOutcome::Failed, Some("设备拒绝")),
            "温度=12.5 [失败: 设备拒绝]"
        );
        assert_eq!(
            write_audit_detail("温度", 12.5, WriteOutcome::Uncertain, Some("无应答")),
            "温度=12.5 [超时: 无应答]"
        );
    }

    #[test]
    fn parse_detail_roundtrips_tri_state() {
        use super::{parse_write_detail, ParsedWriteStatus};
        let ok = parse_write_detail("温度=12.5");
        assert_eq!(ok.name, "温度");
        assert_eq!(ok.value, Some(12.5));
        assert_eq!(ok.status, ParsedWriteStatus::Ok);

        let failed = parse_write_detail("温度=12.5 [失败: 设备拒绝]");
        assert_eq!(failed.value, Some(12.5));
        assert_eq!(failed.status, ParsedWriteStatus::Failed);

        let uncertain = parse_write_detail("温度=12.5 [超时: 无应答]");
        assert_eq!(uncertain.value, Some(12.5));
        assert_eq!(uncertain.status, ParsedWriteStatus::Uncertain);
    }

    #[test]
    fn parse_detail_never_falls_back_to_zero() {
        use super::{parse_write_detail, ParsedWriteStatus};
        // 值解析失败：必须显式标未知、值为 None，不能是「成功、0」
        let parsed = parse_write_detail("温度=not-a-number");
        assert_eq!(parsed.value, None);
        assert_eq!(parsed.status, ParsedWriteStatus::Uncertain);

        // 点位名含 '='：按最后一个 '=' 拆分，值仍能取到
        let parsed = parse_write_detail("a=b=3");
        assert_eq!(parsed.name, "a=b");
        assert_eq!(parsed.value, Some(3.0));
    }

    /// U-4：审计 detail 追加的 ` [cfg:<n>]` 后缀不得影响写结果解析。
    #[test]
    fn parse_detail_ignores_config_generation_suffix() {
        use super::{parse_write_detail, ParsedWriteStatus};
        let ok = parse_write_detail("温度=12.5 [cfg:7]");
        assert_eq!(ok.name, "温度");
        assert_eq!(ok.value, Some(12.5));
        assert_eq!(ok.status, ParsedWriteStatus::Ok);

        let failed = parse_write_detail("温度=12.5 [失败: 设备拒绝] [cfg:7]");
        assert_eq!(failed.value, Some(12.5));
        assert_eq!(failed.status, ParsedWriteStatus::Failed);

        let uncertain = parse_write_detail("温度=12.5 [超时: 无应答] [cfg:7]");
        assert_eq!(uncertain.value, Some(12.5));
        assert_eq!(uncertain.status, ParsedWriteStatus::Uncertain);
    }

    #[test]
    fn audit_source_prefers_operator() {
        use super::WriteContext;
        assert_eq!(WriteContext::new("api").audit_source(), "api");
        assert_eq!(
            WriteContext::new("api")
                .with_operator(Some("alice".into()))
                .audit_source(),
            "alice"
        );
        // 空白操作人视同未上报
        assert_eq!(
            WriteContext::new("api")
                .with_operator(Some("  ".into()))
                .audit_source(),
            "api"
        );
    }
}
