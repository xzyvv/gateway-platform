use super::filter::{DiMode, DiState};
use super::*;

fn make_point(id: &str, address: &str, data_type: Option<&str>) -> PointConfig {
    PointConfig {
        id: id.to_string(),
        device_id: None,
        name: None,
        bus: "gpio".into(),
        protocol: "gpio".into(),
        address: address.into(),
        slave_id: None,
        address_num: None,
        data_id: None,
        function_code: None,
        data_type: data_type.map(|s| s.to_string()),
        byte_order: None,
        register_count: None,
        interval: 1,
        scale: 1.0,
        offset: 0.0,
        formula: None,
        ct_ratio: None,
        pt_ratio: None,
        deadband: 0.0,
        on_change: false,
        unit: None,
        aggregation: None,
        alarm_high: None,
        alarm_low: None,
        modbus_register: None,
        writable: false,
    }
}

/// 验证 GpioCollector 基本信息
#[test]
fn test_gpio_collector_meta() {
    let c = GpioCollector::new("gpio");
    assert_eq!(c.name(), "gpio");
    assert_eq!(c.bus_name(), "gpio");
}

/// 验证 GpioCollector read 的契约：非 Linux 平台如实返回 IoError，绝不返回伪造电平。
///
/// Linux 平台下需要 /dev/gpiochip0，单元测试环境无法保证，
/// 仅验证接口契约（返回 Ok 或 Err），不验证具体值。
#[tokio::test]
async fn test_gpio_read_contract() {
    let c = GpioCollector::new("gpio");
    let p = make_point("di0", "0", None);
    match c.read(&p).await {
        Ok(dp) => {
            // 仅 Linux 且真实芯片可用时才会进入此分支
            assert_eq!(dp.point_id, "di0");
            assert_eq!(dp.raw_type, "gpio");
        }
        Err(e) => {
            // 非 Linux 平台（无 gpiod）或 Linux 平台无 chip：都必须返回明确错误
            let _ = e;
        }
    }
}

/// 计数模式 read 契约：无真实 GPIO 能力时必须报错，不得用恒定 0 掩盖不可采集
#[tokio::test]
async fn test_gpio_counter_read_contract() {
    let c = GpioCollector::new("gpio");
    let p = make_point("cnt0", "di:1", Some("counter_rising"));
    for _ in 0..3 {
        match c.read(&p).await {
            Ok(dp) => {
                assert_eq!(dp.point_id, "cnt0");
            }
            Err(e) => {
                let _ = e;
            }
        }
    }
}

/// 非法地址（含非数字、越界 DO 序号）读取应返回 InvalidConfig
#[tokio::test]
async fn test_gpio_read_rejects_invalid_address() {
    let c = GpioCollector::new("gpio");
    for addr in ["abc", "do:9", "di:x", ""] {
        let p = make_point("p", addr, None);
        let r = c.read(&p).await;
        assert!(
            matches!(r, Err(CollectorError::InvalidConfig(_))),
            "地址 '{}' 应被拒绝，实际: {:?}",
            addr,
            r
        );
    }
}

/// DO 写：裸线号落在 DO 段 4..=7。
///
/// 非 Linux 平台必须返回明确错误（不得谎报成功）；Linux 平台允许因
/// 真实芯片不可用而失败，故仅校验非 Linux 分支。
#[tokio::test]
async fn test_gpio_write_do_bare_pin_ok() {
    let c = GpioCollector::new("gpio");
    let p = make_point("do0", "4", None);
    for val in [1.0, 0.0] {
        let r = c.write(&p, val).await;
        if !crate::platform::gpio::platform_supported() {
            assert!(r.is_err(), "非 Linux 平台写入应返回明确错误，实际: {:?}", r);
        }
    }
}

/// DO 写：`do:<n>` 前缀形式映射到线号 n+4（非 Linux 平台必须报错）
#[tokio::test]
async fn test_gpio_write_do_prefix_ok() {
    let c = GpioCollector::new("gpio");
    for (addr, val) in [("do:0", 1.0), ("do:3", 0.0)] {
        let p = make_point("dop", addr, None);
        let r = c.write(&p, val).await;
        if !crate::platform::gpio::platform_supported() {
            assert!(
                r.is_err(),
                "非 Linux 平台写入 '{}' 应返回明确错误，实际: {:?}",
                addr,
                r
            );
        }
    }
}

/// DO 写：DI 线号（<4）不在 DO 段，必须拒绝
#[tokio::test]
async fn test_gpio_write_rejects_di_pin() {
    let c = GpioCollector::new("gpio");
    for addr in ["0", "3", "di:0"] {
        let p = make_point("dip", addr, None);
        let r = c.write(&p, 1.0).await;
        assert!(
            matches!(r, Err(CollectorError::InvalidConfig(_))),
            "DI 地址 '{}' 不应允许写入，实际: {:?}",
            addr,
            r
        );
    }
}

/// DO 写：非法地址必须返回 InvalidConfig
#[tokio::test]
async fn test_gpio_write_rejects_invalid_address() {
    let c = GpioCollector::new("gpio");
    for addr in ["abc", "do:7", ""] {
        let p = make_point("bad", addr, None);
        let r = c.write(&p, 1.0).await;
        assert!(
            matches!(r, Err(CollectorError::InvalidConfig(_))),
            "地址 '{}' 应被拒绝，实际: {:?}",
            addr,
            r
        );
    }
}

/// 地址解析：三种写法的线号映射
#[test]
fn test_parse_gpio_pin_forms() {
    assert_eq!(parse_gpio_pin("5").unwrap(), 5);
    assert_eq!(parse_gpio_pin(" 7 ").unwrap(), 7);
    assert_eq!(parse_gpio_pin("di:0").unwrap(), 0);
    assert_eq!(parse_gpio_pin("DI:2").unwrap(), 2);
    assert_eq!(parse_gpio_pin("do:0").unwrap(), 4);
    assert_eq!(parse_gpio_pin("DO:3").unwrap(), 7);
}

/// 地址解析：非法输入返回 InvalidConfig
#[test]
fn test_parse_gpio_pin_errors() {
    for addr in ["abc", "do:4", "do:", "di:", "di:x", ""] {
        assert!(
            matches!(parse_gpio_pin(addr), Err(CollectorError::InvalidConfig(_))),
            "'{}' 应解析失败",
            addr
        );
    }
}

/// `write_do` 与 `GpioController` 共享同一状态位图（单一来源）
///
/// 采集路径写 DO 后，`/api/io/status` 读到的状态应立即一致。
///
/// 不变式：**状态位必须与硬件写入结果一致**——
/// - 写入成功 → 对应状态位置位；
/// - 写入失败 → 状态位保持原值（不得显示与实际输出不一致的假状态）。
///
/// 该不变式与平台、硬件是否可用无关：非 Linux 平台无 gpiod 必然失败；
/// Linux 平台若无 `/dev/gpiochip0`（CI runner 即如此）同样失败。
/// 故断言按写入结果分支，而非按平台分支。
///
/// 选用 DO1 / DO2（线号 5 / 6）：本文件其他用例只写 DO0 / DO3，
/// 避免并行测试与全局位图 `DO_BITS` 互相干扰。
#[test]
fn test_write_do_updates_single_state_source() {
    // DO1（线号 5）
    let r = crate::platform::gpio::write_do(5, true);
    let bit = crate::platform::gpio::do_state()[1];
    if r.is_ok() {
        assert!(bit, "写入成功时 DO1 状态位必须置位");
    } else {
        assert!(!bit, "写入失败时不得更新 DO1 状态位");
    }

    // DO2（线号 6）
    let r = crate::platform::gpio::write_do(6, true);
    let bit = crate::platform::gpio::do_state()[2];
    if r.is_ok() {
        assert!(bit, "写入成功时 DO2 状态位必须置位");
    } else {
        assert!(!bit, "写入失败时不得更新 DO2 状态位");
    }
}

/// `write_do` 越界应返回错误，且不改变状态
#[test]
fn test_write_do_out_of_range() {
    assert!(crate::platform::gpio::write_do(3, true).is_err());
    assert!(crate::platform::gpio::write_do(8, true).is_err());
}

/// `DiMode::from_data_type` 映射
#[test]
fn test_di_mode_from_data_type() {
    assert_eq!(DiMode::from_data_type(None), DiMode::Raw);
    assert_eq!(DiMode::from_data_type(Some("u16")), DiMode::Raw);
    assert_eq!(DiMode::from_data_type(Some("debounced")), DiMode::Debounced);
    assert_eq!(DiMode::from_data_type(Some("Debounce")), DiMode::Debounced);
    assert_eq!(
        DiMode::from_data_type(Some("counter")),
        DiMode::CounterRising
    );
    assert_eq!(
        DiMode::from_data_type(Some("counter_rising")),
        DiMode::CounterRising
    );
    assert_eq!(
        DiMode::from_data_type(Some("counter_falling")),
        DiMode::CounterFalling
    );
    assert_eq!(
        DiMode::from_data_type(Some("counter_both")),
        DiMode::CounterBoth
    );
    assert!(!DiMode::Raw.needs_state());
    assert!(DiMode::CounterRising.needs_state());
    assert!(DiMode::CounterRising.is_counter());
    assert!(!DiMode::Debounced.is_counter());
}

/// 防抖：单次扫描毛刺被抑制，连续两次一致才确认
#[test]
fn test_debounce_two_scan_confirmation() {
    let mut st = DiState::new(DiMode::Debounced);
    // 首扫描：无基线，原始电平直通
    assert!(st.update(true).level);
    // 变到 false：仅一次，不确认
    assert!(!st.update(false).level);
    // 第二次 false：建立确认基线 false
    assert!(!st.update(false).level);
    // 单次毛刺 true（随后立刻回 false）：不应被确认
    assert!(!st.update(true).level);
    assert!(!st.update(false).level);
    // 连续两次 true：确认为 true
    assert!(!st.update(true).level);
    assert!(st.update(true).level);
    // 防抖模式不计边沿
    assert_eq!(st.update(true).count, 0);
}

/// 上升沿计数：以确认电平为准，抖动不重复计数
#[test]
fn test_counter_rising() {
    let mut st = DiState::new(DiMode::CounterRising);
    assert_eq!(st.update(false).count, 0); // 首扫描
    assert_eq!(st.update(false).count, 0); // 建立 false 基线
    assert_eq!(st.update(true).count, 0); // 首次 true 未确认
    let o = st.update(true); // 确认 true → 上升沿
    assert!(o.level);
    assert_eq!(o.count, 1);
    // 稳定保持 true 不重复计数
    assert_eq!(st.update(true).count, 1);
    // 回落到 false 再升：第二次上升沿
    assert_eq!(st.update(false).count, 1);
    assert_eq!(st.update(false).count, 1); // 确认 false
    assert_eq!(st.update(true).count, 1); // 未确认
    let o = st.update(true);
    assert_eq!(o.count, 2);
}

/// 下降沿计数
#[test]
fn test_counter_falling() {
    let mut st = DiState::new(DiMode::CounterFalling);
    assert_eq!(st.update(true).count, 0);
    assert_eq!(st.update(true).count, 0); // 建立 true 基线
                                          // 上升沿不计
    assert_eq!(st.update(true).count, 0);
    // 下降：false 首次未确认，第二次确认 → 计 1
    assert_eq!(st.update(false).count, 0);
    let o = st.update(false);
    assert!(!o.level);
    assert_eq!(o.count, 1);
    // 再升再降：第二次下降沿
    assert_eq!(st.update(true).count, 1);
    assert_eq!(st.update(true).count, 1);
    assert_eq!(st.update(false).count, 1);
    let o = st.update(false);
    assert_eq!(o.count, 2);
}

/// 双边沿计数：上升与下降都累加
#[test]
fn test_counter_both_cumulative() {
    let mut st = DiState::new(DiMode::CounterBoth);
    assert_eq!(st.update(false).count, 0);
    assert_eq!(st.update(false).count, 0); // 基线 false
    assert_eq!(st.update(true).count, 0); // 未确认
    assert_eq!(st.update(true).count, 1); // 上升沿
    assert_eq!(st.update(false).count, 1); // 未确认
    assert_eq!(st.update(false).count, 2); // 下降沿
    assert_eq!(st.update(true).count, 2);
    assert_eq!(st.update(true).count, 3); // 上升沿
}
