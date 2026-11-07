//! GPIO 控制（§11 启动流程第 4 步 + Web API IO 端点）
//!
//! - DI（数字输入）4 路
//! - DO（数字输出）4 路
//! - RS485 方向控制（DE/RE 引脚）
//!
//! Linux 平台使用 gpiod crate；非 Linux 平台仅提供「如实声明不支持」的空实现：
//! 任何 GPIO 读写都会返回明确错误，**不再返回伪造的成功/0 值**，
//! 避免上层把不可用能力误判为正常（参见 Bug 53 的同类静默失败教训）。
//!
//! # DO 输出状态单一来源
//!
//! DO 输出状态收敛到模块级 `DO_BITS`（4 位位图）。Web 控制路径
//! （`GpioController::set_do`）与采集写路径（`write_do`，由 GPIO 采集器
//! `Collector::write` 调用）都写入该位图，`/api/io/status` 也从此读取，
//! 避免两条路径各自维护副本导致状态分歧（与 Bug 53 同类的静默不一致）。
//!
//! # gpiod API 说明（0.2.x 批量值 API）
//!
//! gpiod 0.2 起不再提供「按线」API（`Chip::open` / `get_line` /
//! `LineRequestFlags` / `line.request()`），改为**一次 request 一批线**：
//! `Chip::new()` → `request_lines(Options::input(..) | Options::output(..))`
//! → `Lines<Input>` / `Lines<Output>`，读取用 `get_values(&mut [bool; N])`、
//! 写入用 `set_values([bool; N])`。
//! 本文件所有 Linux 分支都基于该 API；`Cargo.lock` 固定 `gpiod 0.2.3`。

use std::sync::atomic::{AtomicU8, Ordering};
#[cfg(target_os = "linux")]
use std::sync::Mutex;

#[cfg(target_os = "linux")]
use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};

use crate::engine::error::{GatewayError, GatewayResult};

/// DO 输出状态位图（bit i = DO i 电平，i 属于 0..4）
///
/// 单一来源：`GpioController::set_do`（Web 路径）与 `write_do`（采集路径）
/// 均写入，`do_state` 读取。
static DO_BITS: AtomicU8 = AtomicU8::new(0);

/// 读取 DO 位图快照（索引即 DO 序号，0..4）
pub fn do_state() -> [bool; 4] {
    let bits = DO_BITS.load(Ordering::Relaxed);
    [
        bits & 0b0001 != 0,
        bits & 0b0010 != 0,
        bits & 0b0100 != 0,
        bits & 0b1000 != 0,
    ]
}

/// 更新 DO 位图的第 `index` 位（index 属于 0..4）
fn set_do_bit(index: usize, on: bool) {
    let mask = 1u8 << index;
    if on {
        DO_BITS.fetch_or(mask, Ordering::Relaxed);
    } else {
        DO_BITS.fetch_and(!mask, Ordering::Relaxed);
    }
}

/// 写 DO 输出线并更新单一状态源。
///
/// `pin` 为**绝对线号**，DO 段约定为 `4..=7`（序号 = `pin - 4`），
/// 与 `GpioController::set_do` 的 `index + 4` 映射、以及 Web
/// `/api/io/:index` 语义一致。越界线号返回错误；硬件驱动失败返回错误且
/// **不**更新状态位，确保 `/api/io/status` 不显示与实际输出不一致的假状态。
pub fn write_do(pin: u32, on: bool) -> GatewayResult<()> {
    if !(4..=7).contains(&pin) {
        return Err(GatewayError::NotFound(format!(
            "DO pin {} out of range (expected 4..=7)",
            pin
        )));
    }
    set_gpio_output(pin, on)?;
    set_do_bit((pin - 4) as usize, on);
    Ok(())
}

/// 当前平台是否具备真实 GPIO 能力（仅 Linux + gpiod 支持）
///
/// 前端据此在非 Linux 平台上隐藏/禁用控制入口，而不是把全 false 的
/// 状态当成「信号异常」展示。
pub const fn platform_supported() -> bool {
    cfg!(target_os = "linux")
}

/// IO 状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IoStatus {
    pub di: [bool; 4],
    pub di_quality: [bool; 4],
    pub do_state: [bool; 4],
    /// 平台是否支持 GPIO 读写；false 时其余字段无物理意义，前端应据此禁用入口
    pub platform_supported: bool,
}

/// DI 段线号（绝对线号 0..3）
///
/// 编译条件含 `test`：映射逻辑是纯函数，可在任意平台单测，
/// 避免「Linux 专属分支永久零覆盖」。
#[cfg(any(target_os = "linux", test))]
const DI_PIN_IDS: [u32; 4] = [0, 1, 2, 3];

/// GPIO 控制器
pub struct GpioController {
    /// Linux 平台占位字段。
    ///
    /// 历史实现把 `gpiod::Chip` 与 DI 输入句柄存在**实例**里，导致：
    /// - Web API（`GpioController`）与采集器（`read_gpio_line`）各开各的
    ///   `/dev/gpiochip0` fd，各自 request 同一批物理线 → gpiod chardev
    ///   语义下同一条线**只能被 request 一次**，第二次必然 `EBUSY`；
    /// - DI 采集（`read_gpio_line`）在 Web 控制器已 request 后必然失败，
    ///   表现为「DI 点位永远读不到值」且错误信息被 EBUSY 掩盖。
    ///
    /// 现在 DI 句柄提升为**进程级全局**（`di_lines()`），chip 也统一走
    /// `get_gpio_chip()`，所有路径共用同一份 request，不再重复占用。
    #[cfg(target_os = "linux")]
    _linux: (),
}

impl Drop for GpioController {
    fn drop(&mut self) {
        self.failsafe();
    }
}

impl GpioController {
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "linux")]
            _linux: (),
        }
    }

    pub fn failsafe(&mut self) {
        tracing::warn!("GPIO failsafe: setting all DO to safe state (low)");
        // Bug 158: 旧实现只把内存 `do_state` 置 false，未真正驱动硬件输出——
        // 进程退出（Drop）/掉电时 DO 引脚仍保持原值，工业输出不进安全态。
        // 现逐路将 DO 线（line index = DO 序号 + 4）实际写回 0。
        #[cfg(target_os = "linux")]
        {
            for i in 0..4u32 {
                if let Err(e) = set_gpio_output(i + 4, false) {
                    tracing::warn!("GPIO failsafe: set DO{} low failed: {}", i, e);
                }
            }
        }
        // 同步单一状态源：无论各线驱动是否成功，内存态一并回安全值，
        // 避免 /api/io/status 显示陈旧 DO 状态。
        DO_BITS.store(0, Ordering::Relaxed);
    }

    pub fn init(&mut self) -> GatewayResult<()> {
        #[cfg(target_os = "linux")]
        {
            // 预先打开 chip（失败直接返回错误，让运维看到真实原因）。
            get_gpio_chip()?;

            // 一次性 request 4 条 DI 输入线并存入**进程级全局**句柄。
            // 失败不阻断启动：降级到"按需 request"路径（read_di_levels 内部
            // 仍会把 request 错误传播出去）。
            match ensure_di_lines() {
                Ok(()) => tracing::info!("GPIO init: cached 4 DI input lines (process-wide)"),
                Err(e) => {
                    tracing::warn!("GPIO init: request DI INPUT lines failed: {}", e);
                }
            }
        }
        Ok(())
    }

    pub fn read_di_with_quality(&self) -> ([bool; 4], [bool; 4]) {
        // Bug 47: 原实现 `let quality;` 在 Linux 分支当 chip 不可用或读取失败时
        // 可能未被赋值，导致返回未初始化内存。修复：显式初始化为 [false; 4]，
        // 仅在成功读取时置为 true。
        #[allow(unused_mut)]
        let mut di = [false; 4];
        #[cfg(target_os = "linux")]
        let mut quality = [false; 4];
        // 非 Linux 平台没有任何真实采样，质量位必须为 false；
        // 之前填 true 会让前端把「没有数据」误读成「信号全部正常」。
        #[cfg(not(target_os = "linux"))]
        let quality = [false; 4];

        #[cfg(target_os = "linux")]
        {
            // 走进程级共享句柄：Web API 与采集器共用同一份 request，
            // 杜绝同线重复 request 导致的 EBUSY。
            match read_di_levels() {
                Ok(vals) => {
                    for i in 0..4usize {
                        di[i] = vals[i];
                        quality[i] = true;
                    }
                }
                Err(e) => {
                    tracing::warn!("read DI values failed: {}", e);
                }
            }
        }

        (di, quality)
    }

    pub fn set_do(&mut self, index: usize, on: bool) -> GatewayResult<()> {
        if index >= 4 {
            return Err(GatewayError::NotFound("DO index out of range".to_string()));
        }
        // Bug 53: 硬件不可用时不得静默返回 Ok（掩盖故障）。
        // 统一走 write_do：驱动硬件 + 更新单一状态源，与采集写路径共享同一实现，
        // 硬件失败时返回错误（且不写状态位），保证状态与输出一致。
        write_do((index + 4) as u32, on)
    }

    pub fn status(&self) -> IoStatus {
        let (di, di_quality) = self.read_di_with_quality();
        IoStatus {
            di,
            di_quality,
            do_state: do_state(),
            platform_supported: platform_supported(),
        }
    }
}

impl Default for GpioController {
    fn default() -> Self {
        Self::new()
    }
}

/// 全局复用的 GPIO chip
#[cfg(target_os = "linux")]
static GPIO_CHIP: OnceCell<Mutex<gpiod::Chip>> = OnceCell::new();
// Mutex 与 OnceCell 的导入见文件顶部：Linux 段需要 `std::sync::Mutex`
// （gpiod::Chip 非 Send，parking_lot::Mutex 在 .await 跨持有时会有问题，故选 std），
// 共享 `once_cell::sync::OnceCell` 是为全局单次初始化。原先仅在 Linux cfg 段使用，
// 但缺导入导致 `cargo check --target aarch64-unknown-linux-gnu` 直接 E0433。

#[cfg(target_os = "linux")]
fn get_gpio_chip() -> GatewayResult<&'static Mutex<gpiod::Chip>> {
    GPIO_CHIP.get_or_try_init(|| {
        let chip = gpiod::Chip::new("/dev/gpiochip0")
            .map_err(|e| GatewayError::Io(format!("open gpiochip0 failed: {}", e)))?;
        Ok(Mutex::new(chip))
    })
}

/// 进程级共享的 DI 输入句柄（4 条线）。
///
/// # 为什么必须是进程级共享（r15-GPIO3 fix）
///
/// gpiod chardev 语义：**同一条物理线在同一时刻只能被 request 一次**，
/// 第二次 request 返回 `EBUSY`。历史实现把 DI 句柄存在 `GpioController`
/// 实例里，而采集器 `read_gpio_line()` 走的是另一条路径再 request 一遍，
/// 于是：
/// - Web API 一旦 `init()` 成功，采集器的 DI 读取**必然 EBUSY 失败**；
/// - 错误被 `map_err` 成 `CollectorError::IoError` 上抛，现场表现是
///   「DI 点位永远读不到值」，且根因被 EBUSY 掩盖。
///
/// 现在 DI 句柄提升为进程级全局，Web API 与采集器共用同一份 request。
#[cfg(target_os = "linux")]
static DI_LINES: OnceCell<parking_lot::Mutex<Option<gpiod::Lines<gpiod::Input>>>> = OnceCell::new();

#[cfg(target_os = "linux")]
fn di_lines_slot() -> &'static parking_lot::Mutex<Option<gpiod::Lines<gpiod::Input>>> {
    DI_LINES.get_or_init(|| parking_lot::Mutex::new(None))
}

/// DI 线号在 DI_PIN_IDS 中的下标（非 DI 段返回 None）。
///
/// 纯函数：任意平台可测（`test` 编译条件下保留）。
#[cfg(any(target_os = "linux", test))]
fn di_index(pin: u32) -> Option<usize> {
    DI_PIN_IDS.iter().position(|&l| l == pin)
}

/// 确保 DI 输入句柄已建立（幂等）。
///
/// 已有缓存 → 直接返回；否则 request 一次并写入全局缓存。
#[cfg(target_os = "linux")]
fn ensure_di_lines() -> GatewayResult<()> {
    if di_lines_slot().lock().is_some() {
        return Ok(());
    }
    let chip_mutex = get_gpio_chip()?;
    let chip = chip_mutex
        .lock()
        .map_err(|e| GatewayError::Io(format!("gpio chip lock poisoned: {}", e)))?;
    let lines = chip
        .request_lines(gpiod::Options::input(DI_PIN_IDS).consumer("gatewayd"))
        .map_err(|e| {
            GatewayError::Io(format!(
                "gpio request DI INPUT lines failed: {} (lines may be busy or permission denied)",
                e
            ))
        })?;
    *di_lines_slot().lock() = Some(lines);
    Ok(())
}

/// 读取 4 路 DI 电平（共享句柄；未建立时按需建立一次）。
#[cfg(target_os = "linux")]
fn read_di_levels() -> GatewayResult<[bool; 4]> {
    ensure_di_lines()?;
    let guard = di_lines_slot().lock();
    let lines = guard
        .as_ref()
        .ok_or_else(|| GatewayError::Io("gpio DI lines unavailable".to_string()))?;
    let mut vals = [false; 4];
    lines
        .get_values(&mut vals)
        .map_err(|e| GatewayError::Io(format!("get DI values failed: {}", e)))?;
    Ok(vals)
}

/// 设置任意 GPIO 引脚输出电平（用于 RS485 DE/RE 方向控制）
///
/// 复用全局 gpiochip0，避免频繁 open/close。
/// DE 控制：写请求前拉高（发送），写完拉低（接收）。
///
/// 非 Linux 平台返回明确错误（不再 no-op 静默成功）。
///
/// Bug r13-GPIO1 (fix): 旧实现 `let _ = line.request(...)` 静默吞掉 request 错误，
/// 后续 `set_value` 用未 request 的 Line → 静默失败。
/// 改为 `?` 传播错误：request 失败时立即返回 Err，
/// 避免高频 DE/RE 切换 + 进程并发场景下"线被其他进程持有"导致的 EBUSY 被掩盖。
pub fn set_gpio_output(pin: u32, on: bool) -> GatewayResult<()> {
    #[cfg(target_os = "linux")]
    {
        let chip_mutex = get_gpio_chip()?;
        let chip = chip_mutex
            .lock()
            .map_err(|e| GatewayError::Io(format!("gpio chip lock poisoned: {}", e)))?;
        // gpiod 0.2 批量 API：一次 request 一条输出线，写完即释放（句柄 drop）
        let out = chip
            .request_lines(gpiod::Options::output([pin]).consumer("gateway-de"))
            .map_err(|e| {
                GatewayError::Io(format!(
                    "gpio {} request OUTPUT failed: {} (line may be busy or permission denied)",
                    pin, e
                ))
            })?;
        out.set_values([on])
            .map_err(|e| GatewayError::Io(format!("set gpio {} value failed: {}", pin, e)))?;
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pin, on);
        // 非 Linux 没有 gpiod，不能假装成功：返回明确错误，
        // 让上层（Web /api/io、RS485 DE/RE 控制）如实报错而不是静默通过。
        return Err(GatewayError::Io(
            "当前平台不支持 GPIO 输出（需 Linux + gpiod）".to_string(),
        ));
    }
    #[allow(unreachable_code)]
    Ok(())
}

/// 读取指定**绝对线号**的 GPIO 输入电平（用于 GPIO 协议采集）
///
/// 与 `read_gpio` 的区别：本函数接收已解析的线号，供采集器先按地址约定
/// （`di:<n>` / `do:<n>` / 裸线号）解析后再读取。
///
/// Bug r13-GPIO1 (fix): 同上——request 错误用 `?` 传播，不静默吞掉。
pub fn read_gpio_line(pin: u32) -> GatewayResult<u8> {
    #[cfg(target_os = "linux")]
    {
        // r15-GPIO3 fix: DI 段（绝对线号 0..=3）必须复用进程级共享句柄，
        // 否则本函数再 request 一遍同一物理线 → gpiod 返回 EBUSY，
        // 表现为「DI 点位永远读不到值」且根因被 EBUSY 掩盖。
        if let Some(idx) = di_index(pin) {
            let vals = read_di_levels()?;
            return Ok(u8::from(vals[idx]));
        }
        let chip_mutex = get_gpio_chip()?;
        let chip = chip_mutex
            .lock()
            .map_err(|e| GatewayError::Io(format!("gpio chip lock poisoned: {}", e)))?;
        let inp = chip
            .request_lines(gpiod::Options::input([pin]).consumer("gatewayd"))
            .map_err(|e| {
                GatewayError::Io(format!(
                    "gpio {} request INPUT failed: {} (line may be busy or permission denied)",
                    pin, e
                ))
            })?;
        let mut vals = [false; 1];
        inp.get_values(&mut vals)
            .map_err(|e| GatewayError::Io(format!("get value failed: {}", e)))?;
        Ok(u8::from(vals[0]))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pin;
        // 非 Linux 没有 gpiod，不能返回伪造的 0 电平：
        // 采集侧会把 0 当成「低电平」正常值，掩盖「该点位根本无法采集」。
        Err(GatewayError::Io(
            "当前平台不支持 GPIO 读取（需 Linux + gpiod）".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 串行化 DO 位图相关用例（位图是进程级全局状态，测试并行会互相污染）。
    static DO_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// DI 线号 → 共享句柄下标的映射必须稳定（DI 段复用全局句柄的前提）。
    #[test]
    fn di_pin_ids_map_to_sequential_indices() {
        for (idx, pin) in DI_PIN_IDS.iter().enumerate() {
            assert_eq!(di_index(*pin), Some(idx));
        }
    }

    /// 非 DI 段线号（DO 段 4..=7 及更高）不得被误判为 DI 共享句柄，
    /// 否则会读到错误的物理线。
    #[test]
    fn non_di_pins_are_not_mapped() {
        for pin in 4..=32u32 {
            assert_eq!(di_index(pin), None, "pin {} must not map to DI handle", pin);
        }
    }

    /// DO 线号越界必须报错，且**不得**改动状态位图
    /// （避免 /api/io/status 显示与实际输出不一致的假状态）。
    #[test]
    fn write_do_rejects_out_of_range_pin() {
        let _guard = DO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let before = do_state();
        assert!(write_do(3, true).is_err(), "pin 3 is DI, not DO");
        assert!(write_do(8, true).is_err(), "pin 8 is out of DO range");
        assert_eq!(do_state(), before, "state bitmap must be unchanged");
    }

    /// 非 Linux 平台必须如实报错，不得返回伪造电平。
    #[cfg(not(target_os = "linux"))]
    #[test]
    fn read_gpio_line_reports_unsupported_platform() {
        assert!(read_gpio_line(0).is_err());
        assert!(read_gpio_line(4).is_err());
    }

    /// Linux 专属：GPIO 读取必须真的走 Linux（gpiod）分支。
    ///
    /// CI runner 没有 `/dev/gpiochip0`，`get_gpio_chip()` 会失败——但失败原因
    /// 必须是「打不开 gpiochip」，而不是「平台不支持 GPIO」。后者意味着
    /// `cfg(target_os)` 判定错了，Linux 上的 GPIO 采集会全线失效且不报错。
    ///
    /// 有真实硬件的机器上可能读成功，因此只断言错误文案，不断言成败。
    #[cfg(target_os = "linux")]
    #[test]
    fn read_gpio_line_takes_linux_gpiod_path() {
        assert!(platform_supported(), "Linux 上平台应标记为支持 GPIO");

        // pin 4 属于 DO 段（DI 段是 0..=3），走 gpiod request_lines 路径
        match read_gpio_line(4) {
            Ok(v) => assert!(v <= 1, "电平只能是 0/1，实际 {v}"),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    !msg.contains("当前平台不支持 GPIO 读取"),
                    "Linux 上不得返回「平台不支持 GPIO」，实际: {msg}"
                );
            }
        }
    }

    /// DO 状态位图的置位/清零必须互不影响。
    #[test]
    fn do_bitmap_set_and_clear_are_independent() {
        let _guard = DO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_do_bit(0, true);
        set_do_bit(2, true);
        let s = do_state();
        assert_eq!(s, [true, false, true, false]);
        set_do_bit(0, false);
        let s = do_state();
        assert_eq!(s, [false, false, true, false]);
        // 复位，避免污染其它用例
        set_do_bit(2, false);
        assert_eq!(do_state(), [false, false, false, false]);
    }
}
