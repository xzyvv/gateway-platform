//! BACnet 协议测试（按主题拆分：golden / write_rpm / cov_discovery / regression）

use super::*;

mod cov_discovery;
mod golden;
mod regression;
mod write_rpm;

/// 构造一个 ReadProperty ComplexACK 响应数据报（BVLC + NPDU + APDU）
fn build_read_property_ack(invoke_id: u8, value: f32) -> Vec<u8> {
    let mut apdu = vec![
        0x30,
        invoke_id,
        SERVICE_READ_PROPERTY,
        0x0C,
        0x00,
        0x00,
        0x00,
        0x01, // ctx tag 0: Object ID (len 4)
        0x19,
        0x55, // ctx tag 1: Property ID=85 (len 1)
        0x3E, // ctx tag 3 opening
        (TAG_REAL << 4) | 4,
    ];
    apdu.extend_from_slice(&value.to_be_bytes());
    apdu.push(0x3F); // ctx tag 3 closing

    let npdu = BacnetCollector::build_npdu_header();
    let mut packet = vec![0x81, 0x0a];
    let total = npdu.len() + apdu.len() + 4;
    packet.extend_from_slice(&(total as u16).to_be_bytes());
    packet.extend_from_slice(&npdu);
    packet.extend_from_slice(&apdu);
    packet
}

/// 构造 RPM-ACK 数据报（BVLC + NPDU + ComplexACK）
fn build_rpm_ack(invoke_id: u8, entries: &[(u16, u32, f32)]) -> Vec<u8> {
    let mut apdu = vec![0x30, invoke_id, SERVICE_READ_PROPERTY_MULTIPLE];
    for (ot, inst, value) in entries {
        let id_val = ((*ot as u32) << 22) | (*inst & 0x3FFFFF);
        apdu.push(0x0C); // ctx tag 0 (objectIdentifier), len 4
        apdu.extend_from_slice(&id_val.to_be_bytes());
        apdu.push(0x1E); // ctx tag 1 opening: listOfResults
        apdu.extend_from_slice(&[0x29, 0x55]); // ctx tag 2 (propertyIdentifier), len 1 = 85
        apdu.push(0x4E); // ctx tag 4 opening: propertyValue
        apdu.push(0x44); // application tag 4 (REAL), len 4
        apdu.extend_from_slice(&value.to_be_bytes());
        apdu.push(0x4F); // ctx tag 4 closing
        apdu.push(0x1F); // ctx tag 1 closing
    }
    let npdu = BacnetCollector::build_npdu_header();
    let mut packet = vec![0x81, 0x0a];
    let total = npdu.len() + apdu.len() + 4;
    packet.extend_from_slice(&(total as u16).to_be_bytes());
    packet.extend_from_slice(&npdu);
    packet.extend_from_slice(&apdu);
    packet
}

/// 构造标准 I-Am 请求 APDU（Device:instance，maxAPDU 1024，segmentation 3，vendor）
fn build_i_am_apdu(device_instance: u32, vendor_id: u16) -> Vec<u8> {
    let mut apdu = vec![APDU_UNCONFIRMED_REQUEST, SERVICE_I_AM];
    let oid = (OBJECT_DEVICE as u32) << 22 | (device_instance & 0x3FFFFF);
    apdu.extend(BacnetCollector::encode_context_tag(0, &oid.to_be_bytes()));
    apdu.extend(BacnetCollector::encode_context_tag(1, &[0x04, 0x00])); // maxAPDU=1024
    apdu.extend(BacnetCollector::encode_context_tag(2, &[0x03])); // segmentation=3
    apdu.extend(BacnetCollector::encode_context_tag(
        3,
        &BacnetCollector::encode_unsigned(vendor_id as u32),
    ));
    apdu
}

/// 构造 COVNotification（未确认）APDU：对象 oid 的最新值为 value
fn build_cov_notification_apdu(oid: u32, value: f32) -> Vec<u8> {
    let mut apdu = vec![
        APDU_UNCONFIRMED_REQUEST,
        SERVICE_UNCONFIRMED_COV_NOTIFICATION,
        0x09,
        0x01, // subscriberProcessIdentifier = 1
        0x1C,
        0x02,
        0x00,
        0x00,
        0x01, // initiatingDeviceIdentifier = Device:1
        0x2C, // monitoredObjectIdentifier [2]
    ];
    apdu.extend_from_slice(&oid.to_be_bytes());
    apdu.extend_from_slice(&[0x39, 0x1E]); // timeRemaining = 30
    apdu.extend_from_slice(&[0x4E, 0x09, 0x55, (TAG_REAL << 4) | 4]); // listOfValues + Present_Value
    apdu.extend_from_slice(&value.to_be_bytes());
    apdu.push(0x4F); // closing tag 4
    apdu
}

fn iam(instance: u32, vendor: u16) -> frames::IAmInfo {
    frames::IAmInfo {
        device_instance: instance,
        max_apdu: 1476,
        vendor_id: vendor,
    }
}
