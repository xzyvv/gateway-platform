//! 浸泡测试规模与时序配置。
//!
//! 默认值对应已确认的现场模型：**4 条总线 × 10 从站 × 50 点 = 2000 点**，
//! 南向 5s 周期 / 24h。规模按「总线 × 从站 × 每从站点数」三维给定，
//! 总点数由三者相乘导出（不再单独指定总数后向下摊派，避免摊派取整
//! 让实际规模与预期悄悄偏离）。
//! 全部字段可用环境变量覆盖，便于驱动脚本做短时冒烟。

use std::path::PathBuf;
use std::time::Duration;

/// 浸泡运行配置。
#[derive(Clone, Debug)]
pub struct SoakSpec {
    /// 南向轮询周期（秒，默认 5）
    pub interval_secs: u64,
    /// 总线数（默认 4）
    pub buses: usize,
    /// 每总线从站数（默认 10 → 40 台设备）
    pub slaves_per_bus: usize,
    /// 每从站点数（默认 50；单从站总点位数 = 该值 × 寄存器宽度）
    pub points_per_slave: usize,
    /// 串口波特率（默认 9600）
    pub baud: u32,
    /// 总线超时（毫秒，默认 2000）
    pub bus_timeout_ms: u64,
    /// 总时长（默认 24h）
    pub duration: Duration,
    /// 预热时长（默认 30min，此段数据不参与判定）
    pub warmup: Duration,
    /// 采样间隔（默认 10s）
    pub sample: Duration,
    /// 输出目录
    pub out_dir: PathBuf,
}

impl SoakSpec {
    pub fn from_env() -> Self {
        let duration_secs = env_u64("SOAK_DURATION_SECS", 86_400);
        // 预热不允超过总时长的 1/5，否则短时冒烟会几乎没有有效数据
        let warmup_default = (duration_secs / 5).min(1_800);
        Self {
            interval_secs: env_u64("SOAK_INTERVAL_SECS", 5).max(1),
            buses: env_u64("SOAK_BUSES", 4).max(1) as usize,
            slaves_per_bus: env_u64("SOAK_SLAVES_PER_BUS", 10).max(1) as usize,
            points_per_slave: env_u64("SOAK_POINTS_PER_SLAVE", 50).max(1) as usize,
            baud: env_u64("SOAK_BAUD", 9_600) as u32,
            bus_timeout_ms: env_u64("SOAK_BUS_TIMEOUT_MS", 2_000),
            duration: Duration::from_secs(duration_secs.max(10)),
            warmup: Duration::from_secs(env_u64("SOAK_WARMUP_SECS", warmup_default)),
            sample: Duration::from_millis(env_u64("SOAK_SAMPLE_MS", 10_000).max(500)),
            out_dir: PathBuf::from(
                std::env::var("SOAK_OUT_DIR").unwrap_or_else(|_| "soak-out".to_string()),
            ),
        }
    }

    /// 每总线点位数。
    pub fn points_per_bus(&self) -> usize {
        self.slaves_per_bus * self.points_per_slave
    }

    /// 实际装配点位数（规模由三维参数直接相乘，无摊派取整误差）。
    pub fn actual_points(&self) -> usize {
        self.buses * self.points_per_bus()
    }

    /// 每个采集周期产生的数据点位数（队列深度阈值以此为量纲）。
    pub fn points_per_cycle(&self) -> usize {
        self.actual_points()
    }

    pub fn device_count(&self) -> usize {
        self.buses * self.slaves_per_bus
    }

    /// 预计采集速率（点/秒），用于报告对照。
    pub fn expected_points_per_sec(&self) -> f64 {
        self.actual_points() as f64 / self.interval_secs as f64
    }
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
