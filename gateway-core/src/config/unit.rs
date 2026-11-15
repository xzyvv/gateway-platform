//! 工程单位：推荐清单与格式校验。
//!
//! 单位只用于展示与 CT/PT 变比的适用范围判定（见 [`crate::config::point::RatioScope`]），
//! 不参与数值解码，因此**不能**做白名单硬校验：早期版本允许自由输入，存量配置里
//! 存在 `kVar` / `m³` / `℃` 这类历史写法，一旦按白名单拒绝，网关会在启动或保存时
//! 直接起不来。这里的职责只有两件事：
//!
//! 1. [`validate_unit`]：挡住明确有害的值（超长、控制字符、首尾空白），这类值会让
//!    上行分组的维度键异常，且没有任何正当用途；
//! 2. [`is_recommended`]：判断是否在推荐清单内，供配置体检给出**告警级**提示，
//!    引导用户改用规范写法而不阻断。
//!
//! 清单必须与前端 `gateway-web/src/composables/devices/useUnits.ts` 的兜底清单
//! 保持一致，并覆盖 `templates/builtin` / `templates/custom` 里实际出现的单位，
//! 否则用模板建出的点位在下拉里没有对应项。

/// 推荐工程单位清单（顺序即前端下拉的分组顺序，按量纲聚拢）。
pub const RECOMMENDED_UNITS: &[&str] = &[
    // 电压
    "V", "kV", "mV", //
    // 电流
    "A", "mA", "kA", //
    // 有功 / 无功 / 视在功率
    "W", "kW", "MW", "var", "kvar", "MVar", "VA", "kVA", //
    // 电能
    "Wh", "kWh", "MWh", "kvarh", //
    // 功率因数（无量纲，变比不参与）
    "PF", "cosφ", //
    // 频率 / 温度
    "Hz", "℃", //
    // 流量 / 体积
    "m3", "m3/h", "L", "L/h", //
    // 压力
    "Pa", "kPa", "MPa", "bar", //
    // 速度 / 转速 / 比例
    "m/s", "rpm", "%", //
    // 时间
    "s", "min", "h", //
    // 其它
    "yuan",
];

/// 单位字符串长度上限（按字符计，够放 `m3/h`、`kvarh` 这类，又挡住误粘贴的长文本）。
pub const MAX_UNIT_LEN: usize = 16;

/// 单位格式校验：只挡明确有害的值，不做白名单。
///
/// 空字符串视为「未填」，放行（等价于 `None`）；调用方应优先存 `None`。
pub fn validate_unit(unit: &str) -> Result<(), String> {
    if unit.is_empty() {
        return Ok(());
    }
    if unit.trim() != unit {
        return Err("单位不能带首尾空白".to_string());
    }
    if unit.chars().count() > MAX_UNIT_LEN {
        return Err(format!("单位长度不能超过 {MAX_UNIT_LEN} 个字符"));
    }
    if unit.chars().any(char::is_control) {
        return Err("单位不能包含控制字符".to_string());
    }
    Ok(())
}

/// 是否在推荐清单内（区分大小写，与前端下拉的取值一致）。
pub fn is_recommended(unit: &str) -> bool {
    RECOMMENDED_UNITS.contains(&unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_recommended_units() {
        for u in ["V", "kWh", "m3/h", "cosφ", "℃", "%"] {
            assert!(validate_unit(u).is_ok(), "{u} 应通过格式校验");
            assert!(is_recommended(u), "{u} 应在推荐清单内");
        }
    }

    #[test]
    fn allows_empty_and_legacy_units() {
        // 空串等价于未填，不能拦
        assert!(validate_unit("").is_ok());
        // 存量历史写法：格式合法，但不推荐（只告警不阻断）
        for u in ["kVar", "m³", "KWH"] {
            assert!(validate_unit(u).is_ok(), "{u} 格式合法，不能拒绝");
            assert!(!is_recommended(u), "{u} 应被判为待规范");
        }
    }

    #[test]
    fn rejects_malformed_units() {
        assert!(validate_unit(" V").is_err(), "首尾空白应拒绝");
        assert!(validate_unit("V ").is_err());
        assert!(validate_unit("V\t").is_err(), "控制字符应拒绝");
        assert!(
            validate_unit(&"x".repeat(MAX_UNIT_LEN + 1)).is_err(),
            "超长应拒绝"
        );
    }
}
