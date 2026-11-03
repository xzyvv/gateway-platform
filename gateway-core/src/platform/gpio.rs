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
#[cfg(target_os = "linux")]
const DI_LINES: [u32; 4] = [0, 1, 2, 3];

/// GPIO 控制器
pub struct GpioController {
    #[cfg(target_os = "linux")]
    chips: std::collections::HashMap<u32, gpiod::Chip>,
    /// Bug r13-GPIO2 (fix): 缓存 4 个 DI 线的批量句柄，避免每次采集周期
    /// (默认 1Hz) 都重新 request + drop——既性能浪费，也与高频
    /// set_gpio_output (DE/RE 切换) 共享 gpiochip0 时偶发 EBUSY
    /// (line 正在被前一次 request 持有)。
    ///
    /// init() 时一次性 request 4 条 DI 输入线并持有到本字段；
    /// read_di_with_quality 直接 get_values，无新 request。
    /// 非 Linux 平台字段类型用 ()，与 cfg(Linux) 字段布局对齐。
    #[cfg(target_os = "linux")]
    di_lines: parking_lot::Mutex<Option<gpiod::Lines<gpiod::Input>>>,
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
            chips: std::collections::HashMap::new(),
            #[cfg(target_os = "linux")]
            di_lines: parking_lot::Mutex::new(None),
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
            let chip = gpiod::Chip::new("/dev/gpiochip0")
                .map_err(|e| GatewayError::Io(format!("open gpiochip0 failed: {}", e)))?;
            self.chips.insert(0, chip);

            // Bug r13-GPIO2 (fix): 初始化时一次性 request 4 条 DI 输入线，
            // 持有批量句柄缓存到 di_lines 字段。
            // 后续 read_di_with_quality 直接 get_values，
            // 不再每次循环 request/drop——既消除 4 次/秒的 sys call 开销，
            // 也消除与高频 set_gpio_output (DE/RE) 共享 chip 时的 EBUSY 竞争。
            //
            // 失败不阻断：request 失败仅 warn，整体降级到"按需 request"路径
            // （降级路径仍把 request 错误传播出去，运维能立即看到问题）。
            let chip = self.chips.get(&0).expect("chip0 just inserted");
            match chip.request_lines(gpiod::Options::input(DI_LINES).consumer("gatewayd")) {
                Ok(lines) => {
                    *self.di_lines.lock() = Some(lines);
                    tracing::info!("GPIO init: cached 4 DI input lines");
                }
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
            // Bug r13-GPIO2 (fix): 优先使用 init() 阶段缓存的批量句柄
            // 直接 get_values()——避免每采集周期 4 次 request+drop 的 sys call 开销
            // 和与高频 set_gpio_output (DE/RE) 共享 chip 时的 EBUSY 竞争。
            // 缓存未命中（init 失败）时降级到"每次 request"路径。
            {
                let cache = self.di_lines.lock();
                if let Some(lines) = cache.as_ref() {
                    let mut vals = [false; 4];
                    match lines.get_values(&mut vals) {
                        Ok(_) => {
                            for i in 0..4usize {
                                di[i] = vals[i];
                                quality[i] = true;
                            }
                            return (di, quality);
                        }
                        Err(e) => {
                            tracing::warn!("read DI cached lines failed: {}", e);
                        }
                    }
                }
            }

            // 降级路径：按需重新 request 4 条输入线后读取。
            // r13-GPIO1 fix：request 错误不再被静默吞掉（记录告警，质量位保持 false）。
            if let Some(chip) = self.chips.get(&0) {
                match chip.request_lines(gpiod::Options::input(DI_LINES).consumer("gatewayd")) {
                    Ok(lines) => {
                        let mut vals = [false; 4];
                        match lines.get_values(&mut vals) {
                            Ok(_) => {
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
                    Err(e) => {
                        tracing::warn!("request DI lines failed: {}", e);
                    }
                }
            } else {
                tracing::warn!("gpiochip0 not available, DI read skipped");
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
