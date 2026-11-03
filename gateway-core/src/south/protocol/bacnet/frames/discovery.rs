//! Who-Is / I-Am 设备发现，SubscribeCOV / COVNotification 编解码。
use super::decode::{parse_app_value_from, read_tag, read_tag_len, read_uint_be};
use super::*;

// 决策表 #21：Who-Is / I-Am 设备发现 + SubscribeCOV / COVNotification
// ============================================================

/// I-Am 报文解析结果（设备发现）
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IAmInfo {
    pub device_instance: u32,
    pub max_apdu: u32,
    pub vendor_id: u16,
}

/// 构建 Who-Is 请求 APDU（Unconfirmed-Request，service 8）
///
/// 给定设备实例号范围时附带 deviceInstanceRangeLowLimit[0] /
/// deviceInstanceRangeHighLimit[1]（各 3 字节）；否则为全局广播 Who-Is。
/// 按 `device_instance` 去重 I-Am 结果，保持首次出现顺序（纯函数，可单测）
///
/// BACnet 设备可能对同一次 Who-Is 广播重复应答（多网卡/多网段），发现列表
/// 必须去重，否则上层「网段设备扫描」会看到同一设备出现多次。
///
/// X4：本函数从 `BacnetCollector::discover_devices` 内联逻辑抽出——后者绑定
/// `0.0.0.0:0` 并广播到 `255.255.255.255:47808`，在 CI（受限网络/无广播
/// 权限）中无法可靠测试，而去重恰是其中唯一的纯逻辑。抽出后生产路径不变，
/// 但去重契约可被直接验证。
#[cfg(test)]
pub(crate) fn dedup_i_am(infos: Vec<IAmInfo>) -> Vec<IAmInfo> {
    let mut out: Vec<IAmInfo> = Vec::with_capacity(infos.len());
    for info in infos {
        if !out
            .iter()
            .any(|x| x.device_instance == info.device_instance)
        {
            out.push(info);
        }
    }
    out
}

pub(crate) fn build_who_is_apdu(low: Option<u32>, high: Option<u32>) -> Vec<u8> {
    let mut apdu = vec![APDU_UNCONFIRMED_REQUEST, SERVICE_WHO_IS];
    if let (Some(l), Some(h)) = (low, high) {
        apdu.extend(BacnetCollector::encode_context_tag(
            0,
            &l.to_be_bytes()[1..],
        ));
        apdu.extend(BacnetCollector::encode_context_tag(
            1,
            &h.to_be_bytes()[1..],
        ));
    }
    apdu
}

/// 解析 I-Am 请求 APDU（Unconfirmed-Request，service 0）
///
/// 布局：0x10 + service(0) + objectIdentifier[0](Device, len4) +
/// maxAPDULengthAccepted[1](unsigned) + segmentationSupported[2](enumerated) +
/// vendorID[3](unsigned)。仅接受 object type = Device(8)。
pub(crate) fn parse_i_am_apdu(apdu: &[u8]) -> CollectorResult<IAmInfo> {
    if apdu.len() < 2 {
        return Err(CollectorError::InvalidFrame("I-Am APDU too short".into()));
    }
    if apdu[0] != APDU_UNCONFIRMED_REQUEST || apdu[1] != SERVICE_I_AM {
        return Err(CollectorError::InvalidFrame(format!(
            "not an I-Am APDU (type 0x{:02X}, service {})",
            apdu[0], apdu[1]
        )));
    }
    let mut pos = 2usize;

    // objectIdentifier [0]，长度 4
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 0 || vf != 4 || p + 4 > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "I-Am: expected objectIdentifier tag 0 (len 4)".into(),
        ));
    }
    let obj = u32::from_be_bytes([apdu[p], apdu[p + 1], apdu[p + 2], apdu[p + 3]]);
    if (obj >> 22) & 0x3FF != OBJECT_DEVICE as u32 {
        return Err(CollectorError::InvalidFrame(
            "I-Am: objectIdentifier is not a Device object".into(),
        ));
    }
    let device_instance = obj & 0x3FFFFF;
    pos = p + 4;

    // maxAPDULengthAccepted [1]
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 1 {
        return Err(CollectorError::InvalidFrame(
            "I-Am: expected maxAPDULengthAccepted tag 1".into(),
        ));
    }
    let (len, ds) = read_tag_len(apdu, p, vf)?;
    if ds + len > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "I-Am: maxAPDU truncated".into(),
        ));
    }
    let max_apdu = read_uint_be(&apdu[ds..ds + len]);
    pos = ds + len;

    // segmentationSupported [2]：当前不消费该值，仅校验并跳过
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 2 {
        return Err(CollectorError::InvalidFrame(
            "I-Am: expected segmentationSupported tag 2".into(),
        ));
    }
    let (len, ds) = read_tag_len(apdu, p, vf)?;
    if ds + len > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "I-Am: segmentationSupported truncated".into(),
        ));
    }
    pos = ds + len;

    // vendorID [3]
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 3 {
        return Err(CollectorError::InvalidFrame(
            "I-Am: expected vendorID tag 3".into(),
        ));
    }
    let (len, ds) = read_tag_len(apdu, p, vf)?;
    if ds + len > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "I-Am: vendorID truncated".into(),
        ));
    }
    let vendor_id = read_uint_be(&apdu[ds..ds + len]) as u16;

    Ok(IAmInfo {
        device_instance,
        max_apdu,
        vendor_id,
    })
}

/// 构建 SubscribeCOV 请求 APDU（Confirmed-Request，service 27）
///
/// 布局：Confirmed-Request(0x00) + MaxSeg/MaxAPDU(0x02) + invoke + service(27) +
/// subscriberProcessIdentifier[0] + monitoredObjectIdentifier[1](len4) +
/// issueConfirmedNotifications[2](boolean) + [lifetime[3](unsigned)，可选]。
///
/// 生产路径：由 `BacnetCollector` 的后台 COV 监听任务发出（订阅专用 socket）。
pub(crate) fn build_subscribe_cov_apdu(
    object_type: u16,
    instance: u32,
    invoke_id: u8,
    subscriber_process_id: u32,
    issue_confirmed: bool,
    lifetime_secs: Option<u32>,
) -> Vec<u8> {
    let mut apdu = vec![
        APDU_CONFIRMED_REQUEST,
        0x02,
        invoke_id,
        SERVICE_SUBSCRIBE_COV,
    ];
    apdu.extend(BacnetCollector::encode_context_tag(
        0,
        &BacnetCollector::encode_unsigned(subscriber_process_id),
    ));
    let oid = ((object_type as u32) << 22) | (instance & 0x3FFFFF);
    apdu.extend(BacnetCollector::encode_context_tag(1, &oid.to_be_bytes()));
    apdu.extend(BacnetCollector::encode_context_tag(
        2,
        &[u8::from(issue_confirmed)],
    ));
    if let Some(lt) = lifetime_secs {
        apdu.extend(BacnetCollector::encode_context_tag(
            3,
            &BacnetCollector::encode_unsigned(lt),
        ));
    }
    apdu
}

/// 解析 SubscribeCOV 响应（仅 SimpleACK(0x20) + 匹配 invoke id + service 27 判成功）
///
/// 生产路径：COV 监听任务用它确认订阅成功，确认后才信任缓存。
pub(crate) fn parse_subscribe_cov_ack(apdu: &[u8], expected_invoke_id: u8) -> CollectorResult<()> {
    if apdu.is_empty() {
        return Err(CollectorError::InvalidFrame("empty BACnet APDU".into()));
    }
    match apdu[0] {
        APDU_SIMPLE_ACK => {
            if apdu.len() < 3 {
                return Err(CollectorError::InvalidFrame(
                    "BACnet SubscribeCOV ACK too short".into(),
                ));
            }
            if apdu[1] != expected_invoke_id {
                return Err(CollectorError::InvalidFrame(format!(
                    "BACnet invoke id mismatch: expected {}, got {}",
                    expected_invoke_id, apdu[1]
                )));
            }
            if apdu[2] != SERVICE_SUBSCRIBE_COV {
                return Err(CollectorError::InvalidFrame(format!(
                    "BACnet unexpected service choice {} (expected SubscribeCOV {})",
                    apdu[2], SERVICE_SUBSCRIBE_COV
                )));
            }
            Ok(())
        }
        APDU_ERROR | APDU_REJECT | APDU_ABORT => {
            let kind = match apdu[0] {
                APDU_ERROR => "Error",
                APDU_REJECT => "Reject",
                _ => "Abort",
            };
            Err(CollectorError::InvalidFrame(format!(
                "BACnet SubscribeCOV rejected: APDU type 0x{:02X} ({})",
                apdu[0], kind
            )))
        }
        other => Err(CollectorError::InvalidFrame(format!(
            "BACnet SubscribeCOV unexpected response type 0x{:02X}",
            other
        ))),
    }
}

/// UnconfirmedCOVNotification（service 2）解析结果
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CovNotification {
    pub subscriber_process_id: u32,
    pub initiating_device_id: u32,
    pub monitored_object_id: u32,
    pub time_remaining: u32,
    pub value: f64,
    pub quality: Quality,
}

/// 解析 COV 通知（UnconfirmedCOVNotification）
///
/// 布局：0x10 + service(2) + subscriberProcessIdentifier[0] +
/// initiatingDeviceIdentifier[1](len4) + monitoredObjectIdentifier[2](len4) +
/// timeRemaining[3] + listOfValues[4]（opening tag 4 + propertyIdentifier[0] +
/// 应用值 + closing tag 4）。
///
/// 生产路径：COV 监听任务用它把推送值写入采集缓存。
pub(crate) fn parse_cov_notification_apdu(apdu: &[u8]) -> CollectorResult<CovNotification> {
    if apdu.len() < 2 {
        return Err(CollectorError::InvalidFrame(
            "COVNotification APDU too short".into(),
        ));
    }
    if apdu[0] != APDU_UNCONFIRMED_REQUEST || apdu[1] != SERVICE_UNCONFIRMED_COV_NOTIFICATION {
        return Err(CollectorError::InvalidFrame(format!(
            "not a COVNotification APDU (type 0x{:02X}, service {})",
            apdu[0], apdu[1]
        )));
    }
    let mut pos = 2usize;

    // subscriberProcessIdentifier [0]
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 0 {
        return Err(CollectorError::InvalidFrame(
            "COV: expected subscriberProcessIdentifier tag 0".into(),
        ));
    }
    let (len, ds) = read_tag_len(apdu, p, vf)?;
    if ds + len > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "COV: subscriberProcessIdentifier truncated".into(),
        ));
    }
    let subscriber_process_id = read_uint_be(&apdu[ds..ds + len]);
    pos = ds + len;

    // initiatingDeviceIdentifier [1]
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 1 || vf != 4 || p + 4 > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "COV: expected initiatingDeviceIdentifier tag 1 (len 4)".into(),
        ));
    }
    let initiating_device_id = u32::from_be_bytes([apdu[p], apdu[p + 1], apdu[p + 2], apdu[p + 3]]);
    pos = p + 4;

    // monitoredObjectIdentifier [2]
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 2 || vf != 4 || p + 4 > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "COV: expected monitoredObjectIdentifier tag 2 (len 4)".into(),
        ));
    }
    let monitored_object_id = u32::from_be_bytes([apdu[p], apdu[p + 1], apdu[p + 2], apdu[p + 3]]);
    pos = p + 4;

    // timeRemaining [3]
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 3 {
        return Err(CollectorError::InvalidFrame(
            "COV: expected timeRemaining tag 3".into(),
        ));
    }
    let (len, ds) = read_tag_len(apdu, p, vf)?;
    if ds + len > apdu.len() {
        return Err(CollectorError::InvalidFrame(
            "COV: timeRemaining truncated".into(),
        ));
    }
    let time_remaining = read_uint_be(&apdu[ds..ds + len]);
    pos = ds + len;

    // listOfValues [4]：opening tag 4 → (propertyIdentifier[0] + 值) → closing tag 4
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 4 || vf != 6 {
        return Err(CollectorError::InvalidFrame(
            "COV: expected opening tag 4 for listOfValues".into(),
        ));
    }
    pos = p;
    // propertyIdentifier[0]：跳过（取 Present_Value 的值本身）
    let (ctx, tag, vf, p) = read_tag(apdu, pos)?;
    if !ctx || tag != 0 {
        return Err(CollectorError::InvalidFrame(
            "COV: expected propertyIdentifier tag 0".into(),
        ));
    }
    let (len, ds) = read_tag_len(apdu, p, vf)?;
    pos = ds + len;
    let (value, quality, _) = parse_app_value_from(apdu, pos)?;

    Ok(CovNotification {
        subscriber_process_id,
        initiating_device_id,
        monitored_object_id,
        time_remaining,
        value,
        quality,
    })
}
