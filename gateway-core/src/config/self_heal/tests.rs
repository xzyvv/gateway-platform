use super::*;

#[test]
fn self_heal_defaults_are_disabled() {
    let c = SelfHealConfig::default();
    assert!(!c.enabled, "自愈默认必须关闭");
    assert_eq!(c.check_interval_secs, 15);
    assert_eq!(c.debounce_k, 3);
    assert_eq!(c.cooldown_secs, 300);
    assert_eq!(c.window_secs, 3600);
    assert_eq!(c.max_actions_per_window, 10);
    assert_eq!(c.fail_streak_to_fuse, 3);
}

#[test]
fn empty_json_object_yields_disabled_defaults() {
    // 配置节存在但字段缺省时，全部取默认值（enabled=false 保持存量零影响）
    let c: SelfHealConfig = serde_json::from_str("{}").expect("empty self_heal should parse");
    assert!(!c.enabled);
    assert_eq!(c.debounce_k, 3);
}
