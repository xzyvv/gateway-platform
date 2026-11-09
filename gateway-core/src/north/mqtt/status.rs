//! 连接级在线状态（静默点判活 Step 1）
//!
//! 这里只表达「网关是否在线」，不涉及点位级静默清单（Step 2）：
//! - 连接时注册 Last Will（retain=true）：broker 在连接**异常**断开时代发 offline；
//! - ConnAck 成功后发布 retain 的 online，覆盖上次异常断开留下的 offline。
//!
//! 状态 topic 固定由上报 topic 派生为 `<topic>/status`，不新增配置字段，
//! 避免配置 schema 迁移、前端表单与生成类型的连锁改动。
//!
//! 关于优雅停机：本仓库从不发送 DISCONNECT 报文，进程退出时 socket 直接关闭，
//! broker 同样按「非正常断开」处理并代发 offline，因此无需在停机路径额外发布。

use std::time::{SystemTime, UNIX_EPOCH};

/// 状态 topic：`factory/line1` → `factory/line1/status`（容忍尾部斜杠）
pub(crate) fn status_topic(base: &str) -> String {
    format!("{}/status", base.trim_end_matches('/'))
}

/// Last Will 负载：**不带时间戳**——will 在连接建立时就被 broker 固化，
/// 带 `ts` 只会显示「连接时刻」，云端会误判离线时间。
pub(crate) fn will_payload() -> Vec<u8> {
    br#"{"state":"offline"}"#.to_vec()
}

/// 在线负载：带毫秒时间戳，便于云端判断该 online 是否新鲜。
pub(crate) fn online_payload() -> Vec<u8> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    format!(r#"{{"state":"online","ts":{ts}}}"#).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_topic_appends_suffix_and_tolerates_trailing_slash() {
        assert_eq!(status_topic("factory/line1"), "factory/line1/status");
        assert_eq!(status_topic("factory/line1/"), "factory/line1/status");
    }

    #[test]
    fn will_payload_is_offline_without_timestamp() {
        let v: serde_json::Value =
            serde_json::from_slice(&will_payload()).expect("will payload is valid json");
        assert_eq!(v["state"], "offline");
        assert!(
            v.get("ts").is_none(),
            "will 负载不能带 ts：will 在连接时固化，ts 会显示连接时刻而非断开时刻"
        );
    }

    #[test]
    fn online_payload_has_state_and_fresh_timestamp() {
        let v: serde_json::Value =
            serde_json::from_slice(&online_payload()).expect("online payload is valid json");
        assert_eq!(v["state"], "online");
        let ts = v["ts"].as_u64().expect("ts 必须是数字");
        assert!(ts > 0, "online 负载必须带可判断新鲜度的时间戳");
    }
}
