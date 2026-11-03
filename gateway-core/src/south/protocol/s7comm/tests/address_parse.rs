//! 地址解析测试（bus 地址中的 rack/slot 与 point 地址格式）。
use super::*;

#[test]
fn test_parse_addr_plain() {
    let (addr, rack, slot) = address::parse_addr("192.168.1.100:102");
    assert_eq!(addr, "192.168.1.100:102");
    assert_eq!(rack, 0);
    assert_eq!(slot, 2);
}

#[test]
fn test_parse_addr_with_rack_slot() {
    let (addr, rack, slot) = address::parse_addr("192.168.1.100:102?rack=1&slot=3");
    assert_eq!(addr, "192.168.1.100:102");
    assert_eq!(rack, 1);
    assert_eq!(slot, 3);
}

#[test]
fn test_parse_addr_partial_query() {
    let (addr, rack, slot) = address::parse_addr("10.0.0.1:102?slot=1");
    assert_eq!(addr, "10.0.0.1:102");
    assert_eq!(rack, 0);
    assert_eq!(slot, 1);
}

#[test]
fn test_parse_db_dword() {
    let addr = address::parse_address("DB1.DBD0", None).unwrap();
    assert_eq!(addr.area, AREA_DB);
    assert_eq!(addr.db_number, 1);
    assert_eq!(addr.byte_offset, 0);
    assert_eq!(addr.data_type, S7DataType::Real);
}

#[test]
fn test_parse_db_word() {
    let addr = address::parse_address("DB1.DBW10", None).unwrap();
    assert_eq!(addr.area, AREA_DB);
    assert_eq!(addr.db_number, 1);
    assert_eq!(addr.byte_offset, 10);
    assert_eq!(addr.data_type, S7DataType::Int);
}

#[test]
fn test_parse_db_byte() {
    let addr = address::parse_address("DB3.DBB5", None).unwrap();
    assert_eq!(addr.area, AREA_DB);
    assert_eq!(addr.db_number, 3);
    assert_eq!(addr.byte_offset, 5);
    assert_eq!(addr.data_type, S7DataType::Byte);
}

#[test]
fn test_parse_db_bit() {
    let addr = address::parse_address("DB1.DBX0.7", None).unwrap();
    assert_eq!(addr.area, AREA_DB);
    assert_eq!(addr.db_number, 1);
    assert_eq!(addr.byte_offset, 0);
    assert_eq!(addr.bit_offset, 7);
    assert_eq!(addr.data_type, S7DataType::Bit);
}

#[test]
fn test_parse_m_dword() {
    let addr = address::parse_address("MD0", None).unwrap();
    assert_eq!(addr.area, AREA_MARKERS);
    assert_eq!(addr.byte_offset, 0);
    assert_eq!(addr.data_type, S7DataType::Real);
}

#[test]
fn test_parse_m_word() {
    let addr = address::parse_address("MW20", None).unwrap();
    assert_eq!(addr.area, AREA_MARKERS);
    assert_eq!(addr.byte_offset, 20);
    assert_eq!(addr.data_type, S7DataType::Int);
}

#[test]
fn test_parse_m_bit() {
    let addr = address::parse_address("M10.3", None).unwrap();
    assert_eq!(addr.area, AREA_MARKERS);
    assert_eq!(addr.byte_offset, 10);
    assert_eq!(addr.bit_offset, 3);
    assert_eq!(addr.data_type, S7DataType::Bit);
}

#[test]
fn test_parse_i_area() {
    let addr = address::parse_address("IW0", None).unwrap();
    assert_eq!(addr.area, AREA_INPUTS);
    assert_eq!(addr.data_type, S7DataType::Int);

    let addr = address::parse_address("ID4", None).unwrap();
    assert_eq!(addr.area, AREA_INPUTS);
    assert_eq!(addr.byte_offset, 4);
    assert_eq!(addr.data_type, S7DataType::Real);
}

#[test]
fn test_parse_q_area() {
    let addr = address::parse_address("QB0", None).unwrap();
    assert_eq!(addr.area, AREA_OUTPUTS);
    assert_eq!(addr.data_type, S7DataType::Byte);

    let addr = address::parse_address("Q0.0", None).unwrap();
    assert_eq!(addr.area, AREA_OUTPUTS);
    assert_eq!(addr.data_type, S7DataType::Bit);
}

#[test]
fn test_parse_with_data_type_hint() {
    // "DB1.DBD0" 默认是 Real，但 hint="dint" 应覆盖
    let addr = address::parse_address("DB1.DBD0", Some("dint")).unwrap();
    assert_eq!(addr.data_type, S7DataType::DInt);

    let addr = address::parse_address("MW0", Some("word")).unwrap();
    assert_eq!(addr.data_type, S7DataType::Word);
}

#[test]
fn test_parse_invalid_address() {
    assert!(address::parse_address("XYZ", None).is_err());
    assert!(address::parse_address("DB1", None).is_err()); // 缺少 .DBx 后缀
    assert!(address::parse_address("M8.9", None).is_err()); // bit > 7
}
