use super::*;
use crate::config::PointConfig;
use crate::engine::model::{ByteOrder, ModbusDataType};

#[test]
fn test_reorder_bytes_abcd_noop() {
    let bytes = [0x12, 0x34, 0x56, 0x78];
    let out = reorder_bytes(&bytes, ByteOrder::Abcd);
    assert_eq!(out, vec![0x12, 0x34, 0x56, 0x78]);
}

#[test]
fn test_reorder_bytes_dcba_reversed() {
    let bytes = [0x12, 0x34, 0x56, 0x78];
    let out = reorder_bytes(&bytes, ByteOrder::Dcba);
    assert_eq!(out, vec![0x78, 0x56, 0x34, 0x12]);
}

#[test]
fn test_reorder_bytes_cdab_4bytes() {
    let bytes = [0x12, 0x34, 0x56, 0x78];
    let out = reorder_bytes(&bytes, ByteOrder::Cdab);
    assert_eq!(out, vec![0x56, 0x78, 0x12, 0x34]);
}

#[test]
fn test_reorder_bytes_badc_4bytes() {
    let bytes = [0x12, 0x34, 0x56, 0x78];
    let out = reorder_bytes(&bytes, ByteOrder::Badc);
    assert_eq!(out, vec![0x34, 0x12, 0x78, 0x56]);
}

#[test]
fn test_reorder_bytes_cdab_8bytes() {
    let bytes = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let out = reorder_bytes(&bytes, ByteOrder::Cdab);
    assert_eq!(out, vec![0x07, 0x08, 0x05, 0x06, 0x03, 0x04, 0x01, 0x02]);
}

#[test]
fn test_reorder_bytes_short_input_passthrough() {
    let bytes = [0x01, 0x02];
    let out = reorder_bytes(&bytes, ByteOrder::Dcba);
    assert_eq!(out, vec![0x01, 0x02]);
}

#[test]
fn test_decode_u16() {
    let bytes = [0x00, 0x0A];
    let (v, t) = decode_binary_value(&bytes, ModbusDataType::U16).unwrap();
    assert_eq!(v, 10.0);
    assert_eq!(t, "u16");
}

#[test]
fn test_decode_i16_negative() {
    let bytes = [0xFF, 0xF6];
    let (v, t) = decode_binary_value(&bytes, ModbusDataType::I16).unwrap();
    assert_eq!(v, -10.0);
    assert_eq!(t, "i16");
}

#[test]
fn test_decode_u32() {
    let bytes = [0x00, 0x00, 0x01, 0x00];
    let (v, t) = decode_binary_value(&bytes, ModbusDataType::U32).unwrap();
    assert_eq!(v, 256.0);
    assert_eq!(t, "u32");
}

#[test]
fn test_decode_i32_negative() {
    let bytes = [0xFF, 0xFF, 0xFF, 0xF6];
    let (v, t) = decode_binary_value(&bytes, ModbusDataType::I32).unwrap();
    assert_eq!(v, -10.0);
    assert_eq!(t, "i32");
}

#[test]
fn test_decode_f32() {
    let bytes = [0x42, 0x48, 0x00, 0x00];
    let (v, t) = decode_binary_value(&bytes, ModbusDataType::F32).unwrap();
    assert!((v - 50.0).abs() < 0.001);
    assert_eq!(t, "f32");
}

#[test]
fn test_decode_u64() {
    let bytes = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0A];
    let (v, t) = decode_binary_value(&bytes, ModbusDataType::U64).unwrap();
    assert_eq!(v, 10.0);
    assert_eq!(t, "u64");
}

#[test]
fn test_decode_f64() {
    let bytes = [0x40, 0x09, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    let (v, t) = decode_binary_value(&bytes, ModbusDataType::F64).unwrap();
    assert!((v - 3.125).abs() < 0.0001);
    assert_eq!(t, "f64");
}

#[test]
fn test_decode_wrong_length_returns_none() {
    let bytes = [0x00, 0x01];
    assert!(decode_binary_value(&bytes, ModbusDataType::U32).is_none());
}

fn make_point_for_convert(
    scale: f64,
    offset: f64,
    formula: Option<String>,
    ct: Option<f64>,
    pt: Option<f64>,
) -> PointConfig {
    PointConfig {
        id: "test".into(),
        name: None,
        device_id: None,
        bus: "b".into(),
        protocol: "modbus".into(),
        address: "0".into(),
        slave_id: Some(1),
        address_num: Some(0),
        data_id: None,
        function_code: Some(3),
        data_type: Some("u16".into()),
        byte_order: None,
        register_count: Some(1),
        interval: 1,
        scale,
        offset,
        formula,
        deadband: 0.0,
        on_change: false,
        unit: None,
        aggregation: None,
        alarm_high: None,
        alarm_low: None,
        ct_ratio: ct,
        pt_ratio: pt,
        modbus_register: None,
        writable: false,
    }
}

/// Bug 227 回归：用 `PointConfig::default()`（或 `..Default::default()`）构造的点位
/// 必须线性透传原始值——原 `#[derive(Default)]` 给出 scale=0.0，
/// 会让 `raw * scale + offset` 恒为 0（采集成功、质量 Good，值却是 0）。
#[test]
fn default_point_passes_raw_value_through_convert() {
    let p = PointConfig::default();
    assert_eq!(p.scale, 1.0, "缺省倍率必须为 1.0");
    assert_eq!(convert_value(&p, 42.5), 42.5);
    assert_eq!(convert_value(&p, -7.25), -7.25);
}

#[test]
fn test_convert_value_scale_offset() {
    let p = make_point_for_convert(0.1, 5.0, None, None, None);
    assert!((convert_value(&p, 100.0) - 15.0).abs() < 0.0001);
}

#[test]
fn test_convert_value_ct_ratio() {
    let p = make_point_for_convert(1.0, 0.0, None, Some(30.0), None);
    assert!((convert_value(&p, 5.0) - 150.0).abs() < 0.0001);
}

#[test]
fn test_convert_value_pt_ratio() {
    let p = make_point_for_convert(1.0, 0.0, None, None, Some(100.0));
    assert!((convert_value(&p, 100.0) - 10000.0).abs() < 0.0001);
}

/// 变比按单位门控：电压只吃 PT、电流只吃 CT、功率/电量吃 CT×PT、功率因数都不吃。
///
/// 改造前是无条件连乘，电压点位若误填 CT=30 会被放大 30 倍（现场常见误配）。
#[test]
fn test_convert_value_ratio_scope_by_unit() {
    let mut p = make_point_for_convert(1.0, 0.0, None, Some(30.0), Some(100.0));

    p.unit = Some("V".into());
    assert!((convert_value(&p, 2.0) - 200.0).abs() < 1e-9, "电压只乘 PT");

    p.unit = Some("kV".into());
    assert!(
        (convert_value(&p, 2.0) - 200.0).abs() < 1e-9,
        "大小写/前缀无关"
    );

    p.unit = Some("A".into());
    assert!((convert_value(&p, 2.0) - 60.0).abs() < 1e-9, "电流只乘 CT");

    p.unit = Some("kW".into());
    assert!(
        (convert_value(&p, 2.0) - 6000.0).abs() < 1e-9,
        "功率乘 CT×PT"
    );

    p.unit = Some("kVarh".into());
    assert!(
        (convert_value(&p, 2.0) - 6000.0).abs() < 1e-9,
        "电量乘 CT×PT"
    );

    p.unit = Some("PF".into());
    assert!(
        (convert_value(&p, 0.95) - 0.95).abs() < 1e-9,
        "功率因数不乘变比"
    );
    p.unit = Some("COSΦ".into());
    assert!(
        (convert_value(&p, 0.95) - 0.95).abs() < 1e-9,
        "全大写功率因数同样不乘"
    );

    // 单位为空 / 未知：保持改造前的兼容行为（两者都乘）
    p.unit = None;
    assert!(
        (convert_value(&p, 2.0) - 6000.0).abs() < 1e-9,
        "空单位兼容旧行为"
    );
    p.unit = Some("℃".into());
    assert!(
        (convert_value(&p, 2.0) - 6000.0).abs() < 1e-9,
        "未知单位兼容旧行为"
    );
}

#[test]
fn test_convert_value_formula_takes_priority() {
    let p = make_point_for_convert(0.1, 5.0, Some("raw*0.01".into()), None, None);
    assert!((convert_value(&p, 100.0) - 1.0).abs() < 0.0001);
}

#[test]
fn test_parse_bcd_address_normal() {
    let bytes = parse_bcd_address("12345678", 4).unwrap();
    assert_eq!(bytes, vec![0x78, 0x56, 0x34, 0x12]);
}

#[test]
fn test_parse_bcd_address_padded() {
    let bytes = parse_bcd_address("123", 4).unwrap();
    assert_eq!(bytes, vec![0x23, 0x01, 0x00, 0x00]);
}

#[test]
fn test_parse_bcd_address_too_long() {
    assert!(parse_bcd_address("123456789", 4).is_err());
}

#[test]
fn test_parse_bcd_address_non_digit() {
    assert!(parse_bcd_address("12AB", 2).is_err());
}

/// r24-DEC1：CT × PT 变比放大产生 +Inf 时，必须回退到 raw 而非传播 Inf。
/// 场景：raw=10, ct=1e200, pt=1e200 → 10 * 1e200 * 1e200 = +Inf
#[test]
fn convert_value_ct_pt_overflow_falls_back_to_raw() {
    let p = make_point_for_convert(1.0, 0.0, None, Some(1e200), Some(1e200));
    let out = convert_value(&p, 10.0);
    assert!(out.is_finite(), "变比溢出应回退到 finite 值，实际 {}", out);
    assert_eq!(out, 10.0, "溢出兜底应回退到 raw=10.0");
}

/// r24-DEC1：极端 scale 让 raw * scale 溢出到 +Inf。
/// 场景：raw=1e300, scale=1e10 → 1e300 * 1e10 = +Inf
#[test]
fn convert_value_scale_overflow_falls_back_to_raw() {
    let p = make_point_for_convert(1e10, 0.0, None, None, None);
    let out = convert_value(&p, 1e300);
    assert!(out.is_finite(), "scale 溢出应回退到 finite，实际 {}", out);
    assert_eq!(out, 1e300, "溢出兜底应回退到 raw=1e300");
}

/// r24-DEC1：负向变比/极端 raw 让结果落到 -Inf，也必须兜底。
#[test]
fn convert_value_negative_inf_falls_back_to_raw() {
    // raw=1e200, ct=-1e200 → 1e200 * -1e200 = -1e400 → -Inf（远超 f64::MAX）
    let p = make_point_for_convert(1.0, 0.0, None, Some(-1e200), None);
    let out = convert_value(&p, 1e200);
    assert!(out.is_finite(), "-Inf 应被兜底，实际 {}", out);
    assert_eq!(out, 1e200);
}

// ===== 写路径反向换算（reverse_convert）=====

/// 写入必须与读取对称：读乘了 CT×PT，写就要除回来。
#[test]
fn reverse_convert_divides_out_effective_ratio() {
    let mut p = make_point_for_convert(1.0, 0.0, None, Some(30.0), Some(100.0));

    // 功率点：CT=30、PT=100 → 一次侧 6000 对应二次侧 2
    p.unit = Some("kW".into());
    assert!((reverse_convert(&p, 6000.0) - 2.0).abs() < 1e-9);

    // 电压点：只有 PT 生效
    p.unit = Some("V".into());
    assert!((reverse_convert(&p, 200.0) - 2.0).abs() < 1e-9);

    // 电流点：只有 CT 生效
    p.unit = Some("A".into());
    assert!((reverse_convert(&p, 60.0) - 2.0).abs() < 1e-9);

    // 功率因数：两者都不生效
    p.unit = Some("PF".into());
    assert!((reverse_convert(&p, 0.95) - 0.95).abs() < 1e-9);
}

/// 先除变比再反解 scale/offset：offset 不能被变比一起放大。
#[test]
fn reverse_convert_applies_offset_after_ratio() {
    let mut p = make_point_for_convert(2.0, 5.0, None, Some(10.0), None);
    p.unit = Some("kW".into());
    // 工程量 = raw*2 + 5，一次侧 = 工程量*10
    // 取一次侧 250 → 工程量 25 → raw = (25-5)/2 = 10
    assert!((reverse_convert(&p, 250.0) - 10.0).abs() < 1e-9);
}

/// 变比为 0（历史配置可能存在）时不得除零。
#[test]
fn reverse_convert_skips_non_positive_ratio() {
    let p = make_point_for_convert(1.0, 0.0, None, Some(0.0), Some(100.0));
    assert!((reverse_convert(&p, 5.0) - 5.0).abs() < 1e-9);
}

/// 读写往返一致：读到的值原样写回去应还原成原始寄存器值。
#[test]
fn read_write_round_trip_is_symmetric() {
    let mut p = make_point_for_convert(0.5, 2.0, None, Some(30.0), Some(100.0));
    p.unit = Some("kWh".into());
    let raw = 40.0f64;
    let primary = convert_value(&p, raw);
    assert!((reverse_convert(&p, primary) - raw).abs() < 1e-9);
}

/// 往返一致必须覆盖全部单位门控分支，而不是只测功率。
///
/// 变比范围由 `ratio_scope` 按单位决定，读写两侧共用同一判定；
/// 任一分支漂移都会表现为「写进去的值和读回来差一个变比」。
#[test]
fn read_write_round_trip_is_symmetric_for_every_unit_scope() {
    for unit in ["kW", "V", "A", "PF", "℃", ""] {
        let mut p = make_point_for_convert(0.5, 2.0, None, Some(30.0), Some(100.0));
        p.unit = if unit.is_empty() {
            None
        } else {
            Some(unit.into())
        };
        let raw = 40.0f64;
        let primary = convert_value(&p, raw);
        assert!(
            (reverse_convert(&p, primary) - raw).abs() < 1e-9,
            "单位 {unit:?} 的读写往返不对称: raw={raw} primary={primary}"
        );
    }
}
