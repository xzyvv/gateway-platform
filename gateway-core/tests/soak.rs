//! T3 浸泡测试入口（A–D 阶段骨架）。
//!
//! 与 `load.rs` 的区别：`load` 是短时负载验证，本入口是**长时间浸泡 + 故障编排 +
//! 通过/失败判定**。全部场景 `#[ignore]`，由 `scripts/soak.ps1` 驱动：
//!
//! ```powershell
//! .\scripts\soak.ps1 -DurationSecs 90          # 冒烟
//! .\scripts\soak.ps1 -DurationSecs 86400       # 24h 正式浸泡
//! ```
//!
//! 手动分步运行：
//! ```text
//! cargo test --test soak -- --ignored --exact soak_run --nocapture --test-threads=1
//! cargo test --test soak -- --ignored --exact soak_verdict --nocapture --test-threads=1
//! ```
//!
//! 运行产物统一落在 `SOAK_OUT_DIR`（默认 `./soak-out`）：
//! - `pid.txt`      测试进程 PID，供驱动脚本采样 RSS / 句柄
//! - `metrics.jsonl` 每个采样周期一行指标快照
//! - `events.jsonl` 故障时间线实际发生的事件
//! - `final.json`   结束时的影响面汇总（含每从站成功数）
//! - `report.md`    判定报告（由 soak_verdict 生成）

#![allow(dead_code)]
// 浸泡农场全部走 Modbus RTU（`registry.create("modbus", ...)`）：
// 协议裁剪构建里没有 modbus 采集器，整个测试二进制无从运行，整体剥离。
#![cfg(feature = "modbus")]

#[path = "chaos/harness.rs"]
mod chaos_harness;
#[path = "soak/mod.rs"]
mod soak;

/// 正式浸泡运行入口。时长 / 规模全部由环境变量控制（见 `SoakSpec::from_env`）。
///
/// ```text
/// SOAK_DURATION_SECS=86400 cargo test --test soak -- --ignored --exact soak_run --nocapture --test-threads=1
/// ```
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "长时间浸泡，由 scripts/soak.ps1 驱动"]
async fn soak_run() {
    let spec = soak::spec::SoakSpec::from_env();
    soak::runner::run(spec).await.expect("浸泡运行失败");
}

/// 判定入口：读上一轮产物，生成 report.md；硬失败则测试失败。
#[test]
#[ignore = "判定阶段，由 scripts/soak.ps1 驱动"]
fn soak_verdict() {
    let spec = soak::spec::SoakSpec::from_env();
    let v = soak::runner::check(&spec).expect("判定失败");
    soak::runner::assert_passed(&v);
}

/// 短时冒烟：60s 跑完整链路 + 全套故障编排（按比例压缩）。
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "短时冒烟，手动运行"]
async fn soak_smoke() {
    let mut spec = soak::spec::SoakSpec::from_env();
    spec.duration = std::time::Duration::from_secs(60);
    spec.warmup = std::time::Duration::from_secs(5);
    spec.sample = std::time::Duration::from_millis(2_000);
    soak::runner::run(spec).await.expect("冒烟运行失败");
}
