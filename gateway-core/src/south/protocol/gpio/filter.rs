//! GPIO DI 信号处理：边沿计数 + 两扫描确认防抖
//!
//! 纯状态机，不依赖硬件，便于单元测试（`platform::gpio::read_gpio_line` 在
//! 非 Linux 平台恒返回 0，无法走端到端）。GPIO 采集器为每个启用处理的点位
//! 持有一个 [`DiState`]，逐次扫描调用 [`DiState::update`]，得到稳定电平与
//! 累计边沿数。
//!
//! # 防抖策略
//!
//! 采用「连续两次扫描一致才确认」：新的原始电平必须与**上一次扫描**相同，
//! 才被接受为稳定电平，从而滤除单次扫描内的抖动毛刺。代价是信号须稳定保持
//! ≥2 个采集周期（`interval` 最小 1s）方可被可靠确认。
//!
//! # 计数模式
//!
//! 边沿以**确认后的稳定电平**发生跳变为准，避免抖动被计成多次边沿。
//! 计数为进程内累计值，重启后归零。
//!
//! # 局限
//!
//! 轮询式计数对频率接近或高于采样率的脉冲会漏计（采样定理限制）。高频脉冲
//! 计数需 gpiod 事件驱动（edge event），列为后续项，本批不实现。

/// DI 处理模式（由点位 `data_type` 解析；未知或缺省为原始电平直通）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiMode {
    /// 原始电平直通（与旧行为兼容，不维护状态）
    Raw,
    /// 两扫描确认后的稳定电平（防抖，不计数）
    Debounced,
    /// 累计上升沿数
    CounterRising,
    /// 累计下降沿数
    CounterFalling,
    /// 累计双边沿数
    CounterBoth,
}

impl DiMode {
    /// 从点位 `data_type` 解析模式（大小写不敏感）
    ///
    /// - `"debounced"` / `"debounce"` → [`DiMode::Debounced`]
    /// - `"counter"` / `"counter_rising"` → [`DiMode::CounterRising`]
    /// - `"counter_falling"` → [`DiMode::CounterFalling`]
    /// - `"counter_both"` → [`DiMode::CounterBoth`]
    /// - 其余（含 `None`）→ [`DiMode::Raw`]
    pub(crate) fn from_data_type(data_type: Option<&str>) -> Self {
        match data_type.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("debounced") | Some("debounce") => DiMode::Debounced,
            Some("counter") | Some("counter_rising") => DiMode::CounterRising,
            Some("counter_falling") => DiMode::CounterFalling,
            Some("counter_both") => DiMode::CounterBoth,
            _ => DiMode::Raw,
        }
    }

    /// 是否需要为该点位维护状态（仅 Raw 模式无需）
    pub(crate) fn needs_state(self) -> bool {
        self != DiMode::Raw
    }

    /// 该模式是否按边沿累计计数
    pub(crate) fn is_counter(self) -> bool {
        matches!(
            self,
            DiMode::CounterRising | DiMode::CounterFalling | DiMode::CounterBoth
        )
    }

    /// 判断从 `from` 跳变到 `to` 是否应计入本模式
    fn counts_edge(self, from: bool, to: bool) -> bool {
        if from == to {
            return false;
        }
        match self {
            DiMode::CounterRising => !from && to,
            DiMode::CounterFalling => from && !to,
            DiMode::CounterBoth => true,
            _ => false,
        }
    }
}

/// 单次扫描输出
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiOutput {
    /// 稳定电平（Raw 模式下即本次原始电平）
    pub level: bool,
    /// 累计边沿数（非计数模式恒为 0）
    pub count: u64,
}

/// 单点位 DI 处理状态
#[derive(Debug, Clone)]
pub(crate) struct DiState {
    mode: DiMode,
    /// 上一次扫描的原始电平（None = 尚未扫描过）
    last_raw: Option<bool>,
    /// 已确认的稳定电平（None = 尚未建立基线）
    confirmed: Option<bool>,
    /// 累计边沿数
    count: u64,
}

impl DiState {
    pub(crate) fn new(mode: DiMode) -> Self {
        Self {
            mode,
            last_raw: None,
            confirmed: None,
            count: 0,
        }
    }

    /// 该状态当前的模式
    pub(crate) fn mode(&self) -> DiMode {
        self.mode
    }

    /// 处理一次扫描的原始电平，返回稳定电平与累计计数
    pub(crate) fn update(&mut self, raw: bool) -> DiOutput {
        if self.mode == DiMode::Raw {
            return DiOutput {
                level: raw,
                count: 0,
            };
        }

        // 两扫描确认：仅当本次原始电平与上一次相同才确认为稳定电平
        if let Some(prev) = self.last_raw {
            if prev == raw {
                match self.confirmed {
                    // 首次建立基线：不计边沿（避免开机误计一次）
                    None => self.confirmed = Some(raw),
                    Some(c) if c != raw => {
                        if self.mode.counts_edge(c, raw) {
                            self.count += 1;
                        }
                        self.confirmed = Some(raw);
                    }
                    // 电平未变化：无操作
                    _ => {}
                }
            }
        }
        self.last_raw = Some(raw);

        // 基线未建立时，先按原始电平返回，避免首扫描延迟一个周期
        let level = self.confirmed.unwrap_or(raw);
        DiOutput {
            level,
            count: self.count,
        }
    }
}
