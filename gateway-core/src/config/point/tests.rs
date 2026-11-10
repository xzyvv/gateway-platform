use super::*;

/// 最小合法点位 JSON：name/slave_id/address_num/function_code/register_count/
/// formula/unit/aggregation 无 serde 缺省值（必填，可为 null），其余字段可省略。
///
/// 协议取 `gpio`：它在任何协议裁剪构建里都合法（见 `domain::VALID_PROTOCOLS`），
/// 而 `modbus` 只在 `--features modbus` 时存在。倍率缺省、scale=0 拒绝这些
/// 与协议无关的不变式，不应因裁剪构建而失效。
const MINIMAL_JSON: &str = r#"{
    "id": "p1",
    "name": null,
    "bus": "b1",
    "protocol": "gpio",
    "address": "1",
    "slave_id": null,
    "address_num": null,
    "function_code": null,
    "register_count": null,
    "interval": 1,
    "formula": null,
    "unit": null,
    "aggregation": null
}"#;

/// Bug 227 回归：`Default` 必须与 serde 缺省值逐字段一致。
///
/// 原 `#[derive(Default)]` 让 scale 得到 0.0，而 JSON 缺省是 1.0——
/// 代码构造的点位读路径 `raw * scale + offset` 恒为 0（静默数据错误）。
#[test]
fn default_matches_serde_defaults() {
    let d = PointConfig::default();
    let from_json: PointConfig =
        serde_json::from_str(MINIMAL_JSON).expect("最小点位 JSON 应可反序列化");

    assert_eq!(
        from_json.scale, 1.0,
        "serde 缺省倍率应为 1.0（default_scale）"
    );
    assert_eq!(
        d.scale, 1.0,
        "Default 倍率必须为 1.0；0.0 会把所有采集值归零"
    );

    // 有 serde 缺省值的字段：两种构造路径必须一致
    assert_eq!(d.scale, from_json.scale, "scale");
    assert_eq!(d.offset, from_json.offset, "offset");
    assert_eq!(d.deadband, from_json.deadband, "deadband");
    assert_eq!(d.on_change, from_json.on_change, "on_change");
    assert_eq!(d.writable, from_json.writable, "writable");
    assert_eq!(d.device_id, from_json.device_id, "device_id");
    assert_eq!(d.data_id, from_json.data_id, "data_id");
    assert_eq!(d.data_type, from_json.data_type, "data_type");
    assert_eq!(d.byte_order, from_json.byte_order, "byte_order");
    assert_eq!(d.ct_ratio, from_json.ct_ratio, "ct_ratio");
    assert_eq!(d.pt_ratio, from_json.pt_ratio, "pt_ratio");
    assert_eq!(d.alarm_high, from_json.alarm_high, "alarm_high");
    assert_eq!(d.alarm_low, from_json.alarm_low, "alarm_low");
    assert_eq!(
        d.modbus_register, from_json.modbus_register,
        "modbus_register"
    );
}

/// 契约：Default 构造的点位不得把 scale/offset/deadband 变成"会让值归零"的组合
#[test]
fn default_point_keeps_linear_pass_through() {
    let p = PointConfig::default();
    let raw = 42.5f64;
    assert_eq!(
        raw * p.scale + p.offset,
        raw,
        "缺省倍率下 raw*scale+offset 必须等于原始值"
    );
}

/// Bug 228 回归：显式配置 `"scale": 0` 必须被校验 fail-fast 拒绝。
///
/// scale=0 会让读路径 `raw*scale+offset` 恒等于 offset（静默归零），
/// 且与写路径 `reverse_convert`（把 0 当作"不缩放"直接透传）语义相反。
#[test]
fn explicit_zero_scale_rejected() {
    use validator::Validate;
    let json = MINIMAL_JSON.replace("\"interval\": 1", "\"interval\": 1, \"scale\": 0.0");
    let p: PointConfig =
        serde_json::from_str(&json).expect("scale=0 应能反序列化（拒绝发生在 validate 阶段）");
    assert_eq!(p.scale, 0.0);
    let err = p.validate().expect_err("scale=0 必须校验失败");
    assert!(
        err.to_string().contains("scale"),
        "错误信息应指向 scale 字段，实际: {}",
        err
    );
}

/// 省略 scale（serde 缺省 1.0）必须通过校验，不受 Bug 228 修复影响。
#[test]
fn omitted_scale_passes_validation() {
    use validator::Validate;
    let p: PointConfig = serde_json::from_str(MINIMAL_JSON).expect("最小点位 JSON 应可反序列化");
    assert_eq!(p.scale, 1.0);
    assert!(p.validate().is_ok(), "缺省 scale=1.0 应通过校验");
}

/// CT/PT 变比必须是有限正数：0 会把一次侧值乘成 0，负数反号。
#[test]
fn non_positive_ratio_rejected() {
    use validator::Validate;
    let with_ratio = |k: &str, v: &str| {
        let json =
            MINIMAL_JSON.replace("\"interval\": 1", &format!("\"interval\": 1, \"{k}\": {v}"));
        serde_json::from_str::<PointConfig>(&json).expect("变比字段应可反序列化")
    };

    assert!(
        with_ratio("ct_ratio", "30.0").validate().is_ok(),
        "ct_ratio=30 合法"
    );
    assert!(
        with_ratio("pt_ratio", "100.0").validate().is_ok(),
        "pt_ratio=100 合法"
    );
    assert!(
        with_ratio("ct_ratio", "0.0").validate().is_err(),
        "ct_ratio=0 必须拒绝"
    );
    assert!(
        with_ratio("pt_ratio", "-1.0").validate().is_err(),
        "pt_ratio=-1 必须拒绝"
    );
}

/// 回收站为空（None）时不得落盘 `point_trash` 键：存量配置零影响。
#[test]
fn empty_trash_is_not_serialized() {
    use crate::config::GatewayConfig;
    let cfg = GatewayConfig::default();
    let v = serde_json::to_value(&cfg).expect("配置应可序列化");
    assert!(
        v.get("point_trash").is_none(),
        "空回收站不应出现在序列化结果中, got {v}"
    );
}

/// 回收站条目可完整往返序列化（含 Unix 毫秒时间戳与原始 PointConfig）。
#[test]
fn trash_entry_roundtrip() {
    use crate::config::GatewayConfig;
    let cfg = GatewayConfig {
        point_trash: Some(vec![PointTrashEntry {
            point: PointConfig {
                id: "p1".into(),
                interval: 10,
                ..Default::default()
            },
            deleted_at_ms: 1_700_000_000_000,
        }]),
        ..Default::default()
    };
    let v = serde_json::to_value(&cfg).expect("配置应可序列化");
    assert_eq!(v["point_trash"][0]["point"]["id"], "p1");
    assert_eq!(v["point_trash"][0]["deleted_at_ms"], 1_700_000_000_000i64);

    let back: GatewayConfig = serde_json::from_value(v).expect("应可反序列化");
    let entry = &back.point_trash.expect("回收站应还原")[0];
    assert_eq!(entry.point.id, "p1");
    assert_eq!(entry.deleted_at_ms, 1_700_000_000_000);
}
