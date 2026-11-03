use super::*;
use crate::engine::model::DataPoint;
use crate::engine::model::Quality;

fn sample_datapoint() -> DataPoint {
    DataPoint {
        point_id: "p1".into(),
        device_id: "d1".into(),
        value: 42.5,
        raw_value: 100.0,
        raw_type: "u16".into(),
        quality: Quality::Good,
        quality_reason: "".into(),
        timestamp: 1700000000,
        source_ts: 1700000000,
        stale_count: 0,
        value_text: None,
    }
}

#[test]
fn test_from_datapoint_maps_all_fields() {
    let dp = sample_datapoint();
    let item = QueuedItem::from_datapoint(&dp);
    assert_eq!(item.id, 0);
    assert_eq!(item.point_id, "p1");
    assert_eq!(item.device_id, "d1");
    assert_eq!(item.value, 42.5);
    assert_eq!(item.raw_value, 100.0);
    assert_eq!(item.raw_type, "u16");
    assert_eq!(item.quality, "good");
    assert_eq!(item.quality_reason, "");
    assert_eq!(item.timestamp, 1700000000);
    assert_eq!(item.source_ts, 1700000000);
    assert_eq!(item.stale_count, 0);
}

#[test]
fn test_queueditem_into_datapoint_quality_mapping() {
    for (q_str, expected) in [
        ("good", Quality::Good),
        ("uncertain", Quality::Uncertain),
        ("bad", Quality::Bad),
        ("stale", Quality::Stale),
        ("unknown", Quality::Good), // 未知默认 Good
    ] {
        let item = QueuedItem {
            id: 1,
            point_id: "p".into(),
            device_id: "d".into(),
            value: 1.0,
            raw_value: 1.0,
            raw_type: "u16".into(),
            quality: q_str.into(),
            quality_reason: "r".into(),
            timestamp: 1,
            source_ts: 1,
            stale_count: 0,
            value_text: None,
        };
        let dp = DataPoint::from(&item);
        assert_eq!(dp.quality, expected, "quality string '{}'", q_str);
        assert_eq!(dp.point_id, "p");
        assert_eq!(dp.value, 1.0);
        assert_eq!(dp.quality_reason, "r");
    }
}

#[test]
fn test_roundtrip_datapoint_queueditem() {
    let dp = sample_datapoint();
    let item = QueuedItem::from_datapoint(&dp);
    let dp2 = DataPoint::from(&item);
    assert_eq!(dp2.point_id, dp.point_id);
    assert_eq!(dp2.device_id, dp.device_id);
    assert_eq!(dp2.value, dp.value);
    assert_eq!(dp2.raw_value, dp.raw_value);
    assert_eq!(dp2.quality, dp.quality);
}

/// 批次 C：`from_datapoint` 必须带上枚举文本标签，否则入队即丢。
#[test]
fn test_from_datapoint_carries_value_text() {
    let mut dp = sample_datapoint();
    dp.value_text = Some("运行".into());
    let item = QueuedItem::from_datapoint(&dp);
    assert_eq!(item.value_text.as_deref(), Some("运行"));
}

/// 批次 C：`From<&QueuedItem> for DataPoint` 必须回填枚举文本标签。
#[test]
fn test_queueditem_into_datapoint_carries_value_text() {
    let mut item = QueuedItem::from_datapoint(&sample_datapoint());
    item.value_text = Some("停机".into());
    let dp = DataPoint::from(&item);
    assert_eq!(dp.value_text.as_deref(), Some("停机"));
}

/// 批次 C：无标签时保持 None（不凭空造空串）。
#[test]
fn test_value_text_none_when_absent() {
    let dp = sample_datapoint();
    let item = QueuedItem::from_datapoint(&dp);
    assert_eq!(item.value_text, None);
    assert_eq!(DataPoint::from(&item).value_text, None);
}

/// 批次 C：序列化风格——有标签时输出 `value_text` 键，无标签时省略该键
/// （/api/history 直接返回 `Vec<QueuedItem>`，响应结构语义不变）。
#[test]
fn test_value_text_serialization_style() {
    let mut item = QueuedItem::from_datapoint(&sample_datapoint());
    item.value_text = Some("运行".into());
    let with_text = serde_json::to_string(&item).expect("serialize item with value_text");
    assert!(
        with_text.contains("\"value_text\":\"运行\""),
        "有标签时应输出 value_text 键，实际: {}",
        with_text
    );

    item.value_text = None;
    let without_text = serde_json::to_string(&item).expect("serialize item without value_text");
    assert!(
        !without_text.contains("value_text"),
        "无标签时应省略 value_text 键，实际: {}",
        without_text
    );
}
