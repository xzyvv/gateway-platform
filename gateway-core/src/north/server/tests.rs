//! 北向 Server 单元测试
//!
//! 统一存放 server 模块所有测试：
//! - mod.rs: Server trait / ModbusServerConfig / RegisterMapping
//! - modbus_server.rs: ModbusServer 读写逻辑
//! - opcua_server.rs: OPC UA 节点管理
//! - mqtt_packet_passthrough.rs: 透传请求/响应序列化
//! - modbus_gateway_bridge.rs: RTU/TCP 帧转换

use super::modbus_gateway_bridge::*;
use super::modbus_server::*;
use super::mqtt_packet_passthrough::*;
#[cfg(feature = "opcua-server")]
use super::opcua_server::*;
use super::*;
#[cfg(feature = "opcua-server")]
use crate::engine::model::DataPoint;

// ===== mod.rs: Server trait / 配置 / 映射 =====

#[test]
fn default_server_config() {
    let cfg = ModbusServerConfig::default();
    assert!(!cfg.enabled);
    assert_eq!(cfg.endpoint, "tcp://0.0.0.0:502");
}

#[test]
fn mapping_from_point() {
    let p = PointConfig {
        id: "temp1".into(),
        scale: 0.1,
        offset: -40.0,
        ..make_min_point()
    };
    let m = RegisterMapping::from_point(&p, 100);
    assert_eq!(m.source_point_id, "temp1");
    assert_eq!(m.register_addr, 100);
    assert_eq!(m.scale, 0.1);
    assert_eq!(m.offset, -40.0);
}

fn make_min_point() -> crate::config::PointConfig {
    crate::config::PointConfig {
        id: "x".into(),
        ..Default::default()
    }
}

// ===== modbus_server.rs: ModbusServer =====

#[test]
fn server_construction() {
    let cfg = ModbusServerConfig {
        enabled: true,
        endpoint: "tcp://127.0.0.1:0".into(),
        ..Default::default()
    };
    let srv = ModbusServer::new(cfg);
    assert!(!srv.is_running());
}

#[test]
fn bind_addr_strip() {
    assert_eq!(ModbusServer::bind_addr("tcp://0.0.0.0:502"), "0.0.0.0:502");
    assert_eq!(ModbusServer::bind_addr("0.0.0.0:502"), "0.0.0.0:502");
}

#[test]
fn set_register() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    srv.set_register(100, 1234);
    assert_eq!(srv.registers.read().get(&100).copied(), Some(1234));
}

#[test]
fn handle_read_fc03() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    srv.set_register(0, 0x1111);
    srv.set_register(1, 0x2222);
    srv.set_register(2, 0x3333);

    let req = [0x00, 0x00, 0x00, 0x03];
    let resp = ModbusServer::handle_read(FC_READ_HOLDING, &req, &srv.registers);
    assert_eq!(resp[0], FC_READ_HOLDING);
    assert_eq!(resp[1], 6);
    assert_eq!(&resp[2..4], &[0x11, 0x11]);
    assert_eq!(&resp[4..6], &[0x22, 0x22]);
    assert_eq!(&resp[6..8], &[0x33, 0x33]);
}

#[test]
fn handle_read_zero_quantity() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    let req = [0x00, 0x00, 0x00, 0x00];
    let resp = ModbusServer::handle_read(FC_READ_HOLDING, &req, &srv.registers);
    assert_eq!(resp[0], FC_READ_HOLDING | 0x80);
    assert_eq!(resp[1], EX_ILLEGAL_DATA_VALUE);
}

#[test]
fn handle_read_too_many() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    let req = [0x00, 0x00, 0x00, 0x7E];
    let resp = ModbusServer::handle_read(FC_READ_HOLDING, &req, &srv.registers);
    assert_eq!(resp[0], FC_READ_HOLDING | 0x80);
    assert_eq!(resp[1], EX_ILLEGAL_DATA_VALUE);
}

#[test]
fn handle_read_missing_registers() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    let req = [0x00, 0x00, 0x00, 0x02];
    let resp = ModbusServer::handle_read(FC_READ_HOLDING, &req, &srv.registers);
    assert_eq!(resp[1], 4);
    assert_eq!(&resp[2..6], &[0, 0, 0, 0]);
}

#[test]
fn handle_write_fc06() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    let req = [0x00, 0x0A, 0xAB, 0xCD];
    let resp = ModbusServer::handle_write_single(FC_WRITE_SINGLE, &req, &srv.registers);
    assert_eq!(resp[0], FC_WRITE_SINGLE);
    assert_eq!(&resp[1..3], &[0x00, 0x0A]);
    assert_eq!(&resp[3..5], &[0xAB, 0xCD]);
    assert_eq!(srv.registers.read().get(&10).copied(), Some(0xABCD));
}

#[test]
fn handle_unsupported_fc() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    // 0x05（写单线圈）未实现，应返回非法功能；FC16 已支持，不能再用作反例。
    let resp = ModbusServer::handle_request(0x05, &[], &srv.registers);
    assert_eq!(resp[0], 0x05 | 0x80);
    assert_eq!(resp[1], EX_ILLEGAL_FUNCTION);
}

#[test]
fn handle_read_bits_fc01() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    // 线圈地址 = 寄存器地址，取最低位
    srv.set_register(0, 0x0001); // on
    srv.set_register(1, 0x0000); // off
    srv.set_register(2, 0x0003); // on（最低位为 1）
    srv.set_register(3, 0x0002); // off（最低位为 0）

    let req = [0x00, 0x00, 0x00, 0x08];
    let resp = ModbusServer::handle_read_bits(FC_READ_COILS, &req, &srv.registers);
    assert_eq!(resp[0], FC_READ_COILS);
    assert_eq!(resp[1], 1);
    // 0b0000_0101：地址 0 与 2 为 on
    assert_eq!(resp[2], 0b0000_0101);
}

#[test]
fn handle_read_bits_fc02() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    srv.set_register(10, 1);
    srv.set_register(11, 0);

    let req = [0x00, 0x0A, 0x00, 0x02];
    let resp = ModbusServer::handle_read_bits(FC_READ_DISCRETE, &req, &srv.registers);
    assert_eq!(resp[0], FC_READ_DISCRETE);
    assert_eq!(resp[1], 1);
    assert_eq!(resp[2], 0b0000_0001);
}

#[test]
fn handle_read_bits_rejects_bad_quantity() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    for qty in [0u16, 2001] {
        let req = [0x00, 0x00, qty.to_be_bytes()[0], qty.to_be_bytes()[1]];
        let resp = ModbusServer::handle_read_bits(FC_READ_COILS, &req, &srv.registers);
        assert_eq!(resp[0], FC_READ_COILS | 0x80);
        assert_eq!(resp[1], EX_ILLEGAL_DATA_VALUE);
    }
}

#[test]
fn handle_write_fc16() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    // addr=10, qty=2, byte_count=4, values=0x1111 0x2222
    let req = [0x00, 0x0A, 0x00, 0x02, 0x04, 0x11, 0x11, 0x22, 0x22];
    let resp = ModbusServer::handle_write_multiple(FC_WRITE_MULTIPLE, &req, &srv.registers);
    assert_eq!(resp[0], FC_WRITE_MULTIPLE);
    assert_eq!(&resp[1..3], &[0x00, 0x0A]);
    assert_eq!(&resp[3..5], &[0x00, 0x02]);
    assert_eq!(srv.registers.read().get(&10).copied(), Some(0x1111));
    assert_eq!(srv.registers.read().get(&11).copied(), Some(0x2222));
}

#[test]
fn handle_write_fc16_rejects_malformed() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    // quantity 为 0
    let req = [0x00, 0x0A, 0x00, 0x00, 0x00];
    assert_eq!(
        ModbusServer::handle_write_multiple(FC_WRITE_MULTIPLE, &req, &srv.registers),
        vec![FC_WRITE_MULTIPLE | 0x80, EX_ILLEGAL_DATA_VALUE]
    );
    // quantity 超过 123
    let req = [0x00, 0x00, 0x00, 0x7C, 0xF8];
    assert_eq!(
        ModbusServer::handle_write_multiple(FC_WRITE_MULTIPLE, &req, &srv.registers),
        vec![FC_WRITE_MULTIPLE | 0x80, EX_ILLEGAL_DATA_VALUE]
    );
    // byte_count 与 quantity 不一致
    let req = [0x00, 0x00, 0x00, 0x02, 0x02, 0x11, 0x11];
    assert_eq!(
        ModbusServer::handle_write_multiple(FC_WRITE_MULTIPLE, &req, &srv.registers),
        vec![FC_WRITE_MULTIPLE | 0x80, EX_ILLEGAL_DATA_VALUE]
    );
    // 数据长度不足
    let req = [0x00, 0x00, 0x00, 0x01, 0x02, 0x11];
    assert_eq!(
        ModbusServer::handle_write_multiple(FC_WRITE_MULTIPLE, &req, &srv.registers),
        vec![FC_WRITE_MULTIPLE | 0x80, EX_ILLEGAL_DATA_VALUE]
    );
}

#[test]
fn handle_request_dispatches_new_function_codes() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    // FC01 走 handle_read_bits
    let req = [0x00, 0x00, 0x00, 0x01];
    assert_eq!(
        ModbusServer::handle_request(FC_READ_COILS, &req, &srv.registers)[0],
        FC_READ_COILS
    );
    // FC16 走 handle_write_multiple
    let req = [0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x05];
    let resp = ModbusServer::handle_request(FC_WRITE_MULTIPLE, &req, &srv.registers);
    assert_eq!(resp[0], FC_WRITE_MULTIPLE);
    assert_eq!(srv.registers.read().get(&0).copied(), Some(5));
}

#[test]
fn exception_format() {
    let resp = ModbusServer::exception(0x03, 0x02);
    assert_eq!(resp, vec![0x83, 0x02]);
}

// ===== opcua_server.rs: OPC UA Server =====

#[cfg(feature = "opcua-server")]
#[test]
fn node_register_and_update() {
    let cfg = OpcUaServerConfig::default();
    let srv = OpcUaServer::new(cfg, None);
    assert_eq!(srv.node_count(), 0);

    srv.register_node("temp1");
    srv.register_node("temp2");
    assert_eq!(srv.node_count(), 2);

    srv.update_value("temp1", 25.5);
    let nodes = srv.nodes.read();
    assert_eq!(nodes.get("temp1").unwrap().value, 25.5);
}

/// 回归测试：OPC UA Server `start()` 必须能真正启动。
///
/// 缺陷背景：builder 未设置 `discovery_urls` 时，async-opcua 的
/// `ServerConfig::validate()` 会报 “Discovery urls not set” 使 `build()` 失败，
/// 服务静默起不来（进程不崩溃、日志还谎报 started）。本测试确保修复不回归。
#[cfg(feature = "opcua-server")]
#[tokio::test]
async fn opcua_server_start_reaches_running() {
    use std::sync::Arc;
    use std::time::Duration;

    // 取一个空闲端口，避免与真实 4840 冲突
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind probe");
    let port = probe.local_addr().expect("local_addr").port();
    drop(probe);

    let cfg = OpcUaServerConfig {
        enabled: true,
        endpoint: format!("opc.tcp://127.0.0.1:{port}"),
        allow_anonymous: false,
        ..Default::default()
    };
    let srv = Arc::new(OpcUaServer::new(cfg, None));
    let srv_run = srv.clone();
    // start() 成功后会阻塞在 server.run()，故放到独立 task，用 running 标志观测
    let run_task = tokio::spawn(async move {
        let _ = srv_run.start().await;
    });

    // 轮询等待进入 running（启动含自签名证书准备，给足时间）
    let mut running = false;
    for _ in 0..50 {
        if srv.is_running() {
            running = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // 无论成败都先停机，避免遗留监听端口
    srv.stop().await.expect("stop");
    let _ = tokio::time::timeout(Duration::from_secs(5), run_task).await;

    assert!(
        running,
        "opcua server 未进入 running：start() 启动失败（discovery_urls 回归）"
    );
}

#[cfg(feature = "opcua-server")]
#[test]
fn apply_points_batch() {
    let cfg = OpcUaServerConfig::default();
    let srv = OpcUaServer::new(cfg, None);
    srv.register_node("a");
    srv.register_node("b");

    let pts = vec![
        DataPoint {
            point_id: "a".into(),
            value: 1.0,
            raw_value: 1.0,
            raw_type: "u16".into(),
            quality: crate::engine::model::Quality::Good,
            timestamp: 0,
            device_id: String::new(),
            quality_reason: String::new(),
            source_ts: 0,
            stale_count: 0,
            value_text: None,
        },
        DataPoint {
            point_id: "b".into(),
            value: 2.0,
            raw_value: 2.0,
            raw_type: "u16".into(),
            quality: crate::engine::model::Quality::Good,
            timestamp: 0,
            device_id: String::new(),
            quality_reason: String::new(),
            source_ts: 0,
            stale_count: 0,
            value_text: None,
        },
    ];
    srv.apply_points(&pts);
    assert_eq!(srv.nodes.read().get("a").unwrap().value, 1.0);
    assert_eq!(srv.nodes.read().get("b").unwrap().value, 2.0);
}

#[cfg(feature = "opcua-server")]
#[test]
fn parse_endpoint_formats() {
    assert_eq!(
        parse_endpoint("opc.tcp://0.0.0.0:4840").unwrap(),
        ("0.0.0.0".to_string(), 4840)
    );
    assert_eq!(
        parse_endpoint("tcp://127.0.0.1:4855").unwrap(),
        ("127.0.0.1".to_string(), 4855)
    );
    assert_eq!(
        parse_endpoint("192.168.1.1:4840").unwrap(),
        ("192.168.1.1".to_string(), 4840)
    );
    assert!(parse_endpoint("no-port").is_err());
}

// ===== passthrough.rs: 透传请求/响应 =====

#[test]
fn test_hex_encode_decode_roundtrip() {
    let original = vec![0x01, 0x03, 0x00, 0x00, 0x00, 0x02, 0xC4, 0x0B];
    let hex = hex_encode(&original);
    assert_eq!(hex, "010300000002c40b");
    let decoded = hex_decode(&hex).unwrap();
    assert_eq!(decoded, original);
}

#[test]
fn test_hex_decode_odd_length() {
    assert!(hex_decode("abc").is_err());
}

#[test]
fn test_hex_decode_invalid_char() {
    assert!(hex_decode("zz").is_err());
}

/// 修复(审查#A2) 回归：data 含多字节字符时不得 panic（原实现按字节下标做
/// `&s[i..i+2]` 字符串切片，索引落在字符边界中间会直接 panic）。
/// "0103中1" 共 8 字节（长度校验通过），旧实现在 i=4 处切片 panic。
#[test]
fn test_hex_decode_multibyte_does_not_panic() {
    assert!(hex_decode("0103中1").is_err());
}

#[test]
fn test_hex_decode_accepts_uppercase() {
    assert_eq!(hex_decode("01FF").unwrap(), vec![0x01, 0xFF]);
}

/// 修复(审查#A6) 回归：超范围/非有限值不得静默饱和成 65535 或 0，
/// 应保留寄存器上一次有效值。
#[test]
fn apply_registers_skips_out_of_range_values() {
    let srv = ModbusServer::new(ModbusServerConfig::default());
    let mappings = vec![RegisterMapping {
        source_point_id: "p1".into(),
        register_addr: 10,
        scale: 1.0,
        offset: 0.0,
    }];
    let write = |v: f64| {
        srv.apply_registers(
            &mappings,
            &std::collections::HashMap::from([("p1".to_string(), v)]),
        );
    };
    write(1234.0);
    assert_eq!(srv.registers.read().get(&10).copied(), Some(1234));
    // 超出 u16 上限：旧实现会写成 65535
    write(70000.0);
    assert_eq!(srv.registers.read().get(&10).copied(), Some(1234));
    // 负值：旧实现会写成 0
    write(-5.0);
    assert_eq!(srv.registers.read().get(&10).copied(), Some(1234));
    // NaN：旧实现会写成 0
    write(f64::NAN);
    assert_eq!(srv.registers.read().get(&10).copied(), Some(1234));
    // 范围内仍正常写入
    write(65535.0);
    assert_eq!(srv.registers.read().get(&10).copied(), Some(65535));
}

#[test]
fn test_request_deserialize() {
    let json = r#"{"bus":"com1","data":"0103","timeout":500,"id":"r1"}"#;
    let req: PassthroughRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.bus, "com1");
    assert_eq!(req.data, "0103");
    assert_eq!(req.timeout, 500);
    assert_eq!(req.id, "r1");
}

#[test]
fn test_request_default_timeout() {
    let json = r#"{"bus":"com1","data":"0103"}"#;
    let req: PassthroughRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.timeout, 2000);
    assert_eq!(req.id, "");
}

#[test]
fn test_response_serialize_ok() {
    let resp = PassthroughResponse {
        id: "r1".into(),
        bus: "com1".into(),
        data: Some("010304".into()),
        ok: true,
        error: None,
        elapsed_ms: 15,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"ok\":true"));
    assert!(json.contains("010304"));
    assert!(!json.contains("error"));
}

#[test]
fn test_response_serialize_error() {
    let resp = PassthroughResponse {
        id: "r1".into(),
        bus: "com1".into(),
        data: None,
        ok: false,
        error: Some("timeout".into()),
        elapsed_ms: 2000,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains("\"ok\":false"));
    assert!(json.contains("timeout"));
}

// ===== modbus_gateway_bridge.rs: RTU/TCP 帧转换 =====

#[test]
fn test_rtu_to_tcp_normal() {
    let rtu = vec![0x01, 0x03, 0x04, 0x00, 0x0A, 0x00, 0x14, 0x71, 0xD3];
    let tcp = rtu_to_tcp(&rtu, 0x1234);
    assert_eq!(&tcp[0..2], &[0x12, 0x34]);
    assert_eq!(&tcp[2..4], &[0x00, 0x00]);
    assert_eq!(&tcp[4..6], &[0x00, 0x07]);
    assert_eq!(tcp[6], 0x01);
    assert_eq!(&tcp[7..], &[0x03, 0x04, 0x00, 0x0A, 0x00, 0x14]);
}

#[test]
fn test_rtu_to_tcp_exception() {
    let rtu = vec![0x01, 0x83, 0x02, 0xC0, 0xF1];
    let tcp = rtu_to_tcp(&rtu, 0x0001);
    assert_eq!(tcp[6], 0x01);
    assert_eq!(tcp[7], 0x83);
    assert_eq!(tcp[8], 0x02);
}

#[test]
fn test_build_exception_response() {
    let tcp = build_exception_response(0x0005, 0x01, 0x03, 0x0B);
    assert_eq!(&tcp[0..2], &[0x00, 0x05]);
    assert_eq!(&tcp[4..6], &[0x00, 0x03]);
    assert_eq!(tcp[6], 0x01);
    assert_eq!(tcp[7], 0x83);
    assert_eq!(tcp[8], 0x0B);
}

// ===== modbus_gateway_bridge.rs: 回归——异常分支必须产出合法 TCP 帧 =====

/// 仅测试用：构建一个「不含任何总线」的 GatewayRuntime，用于驱动透传的 `None` 异常分支。
///
/// 装配实现已移至 `crate::runtime::gateway_runtime_without_buses_for_test`：装配整机必须
/// 具名 `Scheduler` / `SerialManager` 等南向类型，而架构守护
/// `north_must_not_depend_on_south`（gateway-core/tests/arch_deps.rs）禁止 north 出现
/// `crate::south::` 引用。装配属 support 层职责（`GatewayComponents` 定义于此），故收敛到该层。
fn test_gateway_runtime_without_buses() -> std::sync::Arc<crate::runtime::GatewayRuntime> {
    crate::runtime::gateway_runtime_without_buses_for_test()
}

/// 回归缺陷维度：修复前异常分支直接返回 `build_exception_response` 生成的 **TCP** 帧，
/// 又被出口 `rtu_to_tcp` 当作 RTU 二次封装，产出畸形帧
/// （unit_id 取到事务号高字节、长度域算成 7）。
///
/// 本测试驱动真实透传处理器走到 `None`（目标总线不存在）异常分支，读取最终对外
/// 发送的字节，断言它恰好是合法的 TCP 异常帧：
/// `[tid_hi, tid_lo, 0, 3, unit_id, fc|0x80, exception_code]`。
#[tokio::test]
async fn passthrough_missing_bus_emits_wellformed_tcp_exception_frame() {
    use crate::config::ModbusPassthroughConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let port = {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        l.local_addr().unwrap().port()
    };

    let cfg = ModbusPassthroughConfig {
        enabled: true,
        listen_addr: format!("127.0.0.1:{port}"),
        target_bus: "no-such-bus".to_string(),
        timeout_ms: 200,
    };
    let srv = ModbusGatewayBridgeServer::new(cfg, test_gateway_runtime_without_buses());
    let run = srv.clone();
    tokio::spawn(async move {
        let _ = run.start().await;
    });

    // 等待监听就绪
    let mut ready = false;
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(ready, "passthrough 监听未就绪");

    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    // MBAP: tid=0x0102, protocol=0, length=2（含 unit）, unit=0x11, fc=0x03
    stream
        .write_all(&[0x01, 0x02, 0x00, 0x00, 0x00, 0x02, 0x11, 0x03])
        .await
        .unwrap();

    let mut resp = [0u8; 9];
    stream.read_exact(&mut resp).await.unwrap();
    assert_eq!(
        &resp,
        &[0x01, 0x02, 0x00, 0x00, 0x00, 0x03, 0x11, 0x83, 0x04],
        "异常分支对外必须是标准 TCP 帧 [tid,0,0,len=3,unit,fc|0x80,code]，实际 {:02X?}",
        resp
    );

    let _ = srv.stop().await;
}

// ===== r30-PS1/PS2/PS3 回归测试 =====

/// r30-PS1 (回归)：handle_request 入口 try_acquire semaphore，超过上限立刻返回错误。
/// 我们手动持满 16 个 permit（与 MAX_CONCURRENT_PASSTHROUGH 常量一致），
/// 然后再调用 handle_request，期望响应含 "too many concurrent passthrough requests"。
#[tokio::test]
async fn passthrough_rejects_when_semaphore_exhausted() {
    use std::sync::{Arc, Mutex};
    use tokio::sync::Semaphore;
    let concurrency = Arc::new(Semaphore::new(MAX_CONCURRENT_PASSTHROUGH));
    // 拿满 16 个 permit，模拟"已有 16 个并发请求在跑"
    let _held: Vec<_> = (0..MAX_CONCURRENT_PASSTHROUGH)
        .map(|_| concurrency.clone().try_acquire_owned().expect("acquire"))
        .collect();

    let gateway = test_gateway_runtime_without_buses();
    let sessions = Arc::new(Mutex::new(Vec::new()));
    // 任意 payload：semaphore 在 JSON 解析前拦截，payload 内容无关紧要。
    let payload = br#"{"bus":"x","data":"00","timeout":1000}"#;
    let resp =
        MqttPacketPassthrough::handle_request(&gateway, payload, sessions, concurrency).await;
    assert!(!resp.ok, "第 17 个请求应被拒绝");
    let err = resp.error.as_deref().unwrap_or("");
    assert!(
        err.contains("too many concurrent passthrough"),
        "应含限流错误信息，实际: {}",
        err
    );
}

/// r30-PS3 (回归)：data 字段超过 4 KiB hex 字符串直接拒绝，避免先分配大 Vec 再报错。
#[tokio::test]
async fn passthrough_rejects_oversized_data() {
    use std::sync::{Arc, Mutex};
    use tokio::sync::Semaphore;
    let concurrency = Arc::new(Semaphore::new(MAX_CONCURRENT_PASSTHROUGH));
    let gateway = test_gateway_runtime_without_buses();
    let sessions = Arc::new(Mutex::new(Vec::new()));

    // 构造 9000 字节 hex（即 4500 字节二进制），超过 MAX_PASSTHROUGH_BYTES=4096。
    let oversized_data = "AA".repeat(MAX_PASSTHROUGH_BYTES * 2 + 100);
    let payload = format!(
        r#"{{"bus":"x","data":"{}","timeout":1000,"id":"r1"}}"#,
        oversized_data
    );
    let resp =
        MqttPacketPassthrough::handle_request(&gateway, payload.as_bytes(), sessions, concurrency)
            .await;
    assert!(!resp.ok);
    let err = resp.error.as_deref().unwrap_or("");
    assert!(
        err.contains("data too long"),
        "应含 data too long 错误，实际: {}",
        err
    );
    // id / bus 应原样带回，便于客户端关联。
    assert_eq!(resp.id, "r1");
    assert_eq!(resp.bus, "x");
}

/// r30-PS2 (回归)：timeout 上限常量与钳制语义。
/// 不能在 handle_request 中端到端验证（需 mock 慢响应 bus），但可测常量和钳制函数。
#[test]
fn passthrough_timeout_constant_clamp() {
    // 硬上限必须为 30s（PS-2 设计值），防止后续被改大。
    assert_eq!(MAX_PASSTHROUGH_TIMEOUT_MS, 30_000);

    // 模拟 handle_request 内的钳制公式：clamp(1, MAX)
    let clamp = |v: u64| v.clamp(1, MAX_PASSTHROUGH_TIMEOUT_MS);
    // u64::MAX → 钳到 30000
    assert_eq!(clamp(u64::MAX), 30_000);
    // 0 → 钳到 1（防 Duration::from_millis(0) 的边界 case）
    assert_eq!(clamp(0), 1);
    // 正常值不变
    assert_eq!(clamp(2_000), 2_000);
    // 边界值
    assert_eq!(clamp(MAX_PASSTHROUGH_TIMEOUT_MS), 30_000);
    assert_eq!(clamp(MAX_PASSTHROUGH_TIMEOUT_MS + 1), 30_000);
}

/// r30c-PS7 (回归)：空 id 请求的响应 id 应为 `anon-<started_at_ms>`，便于排障。
/// 旧实现直接回 `id=""`，多个空 id 请求彼此不可区分。
#[tokio::test]
async fn passthrough_empty_id_filled_with_anon_prefix() {
    use std::sync::{Arc, Mutex};
    use tokio::sync::Semaphore;
    let concurrency = Arc::new(Semaphore::new(MAX_CONCURRENT_PASSTHROUGH));
    let gateway = test_gateway_runtime_without_buses();
    let sessions = Arc::new(Mutex::new(Vec::new()));

    // id 字段为空字符串。bus="x" 不存在 → 走到 "bus not found" 错误，
    // 但 id 已经被 PS-7 替换成 anon- 前缀。
    let payload = br#"{"bus":"x","data":"00","timeout":1000,"id":""}"#;
    let resp =
        MqttPacketPassthrough::handle_request(&gateway, payload, sessions, concurrency).await;
    assert!(!resp.ok);
    assert!(
        resp.id.starts_with("anon-"),
        "空 id 应被替换为 anon-<ts> 形式，实际: {:?}",
        resp.id
    );
    // 形如 "anon-1234567890123"
    let ts_part = &resp.id[5..];
    assert!(
        ts_part.parse::<u64>().is_ok(),
        "anon- 后应是时间戳数字，实际: {:?}",
        resp.id
    );
}

/// r30c-PS7 补充：非空 id 必须原样保留，不被 PS-7 替换。
#[tokio::test]
async fn passthrough_non_empty_id_preserved() {
    use std::sync::{Arc, Mutex};
    use tokio::sync::Semaphore;
    let concurrency = Arc::new(Semaphore::new(MAX_CONCURRENT_PASSTHROUGH));
    let gateway = test_gateway_runtime_without_buses();
    let sessions = Arc::new(Mutex::new(Vec::new()));
    let payload = br#"{"bus":"x","data":"00","timeout":1000,"id":"req-42"}"#;
    let resp =
        MqttPacketPassthrough::handle_request(&gateway, payload, sessions, concurrency).await;
    assert_eq!(resp.id, "req-42", "非空 id 必须原样保留");
}

/// 回归缺陷维度：两处 `read_exact` 原实现无超时——客户端只发半个 MBAP 头（或半个 PDU）
/// 即可让连接任务永久挂起（`spawn_catch_unwind` 只捕 panic，不回收挂起任务），
/// 连接与任务持续泄漏。
///
/// 本测试向同一处理函数注入短读取超时（500ms），断言：
/// 1) 超时之前连接**不会**被关闭（证明残缺帧确实在等待）；
/// 2) 超过超时后服务端主动关闭连接（客户端读到 EOF）。
/// 修复前第 2 步会一直挂起，由外层 5s 兜底判失败。
#[tokio::test]
async fn passthrough_partial_frame_is_closed_after_read_timeout() {
    use crate::config::ModbusPassthroughConfig;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let cfg = ModbusPassthroughConfig {
        enabled: true,
        listen_addr: addr.to_string(),
        target_bus: "no-such-bus".to_string(),
        timeout_ms: 200,
    };
    let gateway = test_gateway_runtime_without_buses();
    let read_timeout = std::time::Duration::from_millis(500);
    tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        handle_client(sock, &cfg, &gateway, read_timeout).await;
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    // MBAP 头共 7 字节，这里故意只发 3 字节
    client.write_all(&[0x01, 0x02, 0x00]).await.unwrap();

    // 1) 读取超时前：连接保持（read 挂起 → 外层 50ms 兜底超时）
    let mut buf = [0u8; 1];
    let early =
        tokio::time::timeout(std::time::Duration::from_millis(50), client.read(&mut buf)).await;
    assert!(
        early.is_err(),
        "半帧在读取超时前不应被关闭，实际 {:?}",
        early
    );

    // 2) 读取超时后：服务端主动关闭 → 客户端读到 EOF（Ok(0)）
    let closed =
        tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf)).await;
    match closed {
        Ok(Ok(0)) => {}
        Ok(Ok(n)) => panic!("期望服务端关闭连接（EOF），却读到 {} 字节", n),
        Ok(Err(e)) => panic!("期望 EOF，却得到读错误: {}", e),
        Err(_) => panic!("服务端未在读取超时后关闭连接（修复前即永久挂起）"),
    }
}

// ===== r60-SD2 回归：Notify 丢失唤醒修复（与 r48-MQTT1 同型）=====

/// r60-SD2（回归）：`wait_shutdown_notify` 在 stopping 已置位时必须立即返回，
/// 不能 hang 在 notified().await 上。这是修复 r48 同型 bug 的核心契约：
/// `stop()` 在 select! 注册 waiter 之前先置位 `stopping`，waiter 在 `enable()` 之后
/// 立刻复查标志，看到 true 就退出——覆盖"置位与注册之间"的丢失唤醒窗口。
#[tokio::test]
async fn r60_wait_shutdown_notify_returns_immediately_when_stopping_already_set() {
    use super::wait_shutdown_notify;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::Notify;

    let notify = Notify::new();
    let stopping = AtomicBool::new(false);

    // 关键：模拟"stop() 已先置位"——此时 waiter 才注册。
    stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    notify.notify_waiters();

    let started = std::time::Instant::now();
    wait_shutdown_notify(&notify, &stopping).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(50),
        "r60: stopping 已置位时 wait_shutdown_notify 必须立即返回（不 hang 在 notified().await），实际耗时 {:?}",
        elapsed
    );
}

/// r60-SD2（回归）：stopping 置位与 `notify_waiters()` 之间即使间隔极短，
/// 后续 waiter 也能立即收到停机信号。这是 r48-MQTT1 的核心场景：
/// stop() 第一句是 `stopping.store(true)`，第二句是 `notify_waiters()`；
/// 任何在两句之间注册的 waiter 都必须通过复查 stopping 标志立即退出。
#[tokio::test]
async fn r60_wait_shutdown_notify_recovers_from_set_before_register_window() {
    use super::wait_shutdown_notify;
    use std::sync::atomic::AtomicBool;
    use tokio::sync::Notify;

    let notify = Notify::new();
    let stopping = AtomicBool::new(false);

    // 模拟 stop() 路径：先置位 stopping，再 notify_waiters。
    stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    notify.notify_waiters();

    // 此刻 waiter 还未注册——经典丢失唤醒窗口。
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    // 后续 waiter 注册时必须靠 stopping 标志立即返回，不能等永远不来的 notify。
    let started = std::time::Instant::now();
    wait_shutdown_notify(&notify, &stopping).await;
    assert!(
        started.elapsed() < std::time::Duration::from_millis(50),
        "r60: 经典丢失唤醒窗口下 waiter 必须立即返回（实测 {:?}）",
        started.elapsed()
    );
}

/// r60-SD2（回归）：stopping 未置位时 `wait_shutdown_notify` 阻塞等待，
/// 后续 `notify_waiters()` 能正确唤醒。这是 happy path 验证。
#[tokio::test]
async fn r60_wait_shutdown_notify_blocks_then_wakes_on_notify() {
    use super::wait_shutdown_notify;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use tokio::sync::Notify;

    let notify = Arc::new(Notify::new());
    let stopping = Arc::new(AtomicBool::new(false));

    let n2 = notify.clone();
    let s2 = stopping.clone();
    let waiter = tokio::spawn(async move {
        let started = std::time::Instant::now();
        wait_shutdown_notify(&n2, &s2).await;
        started.elapsed()
    });

    // 等 waiter 真的进入 select 块（已 enable waiter）。
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    // 模拟 stop() 路径：先置位再 notify。
    stopping.store(true, std::sync::atomic::Ordering::SeqCst);
    notify.notify_waiters();

    let elapsed = waiter.await.unwrap();
    assert!(
        elapsed >= std::time::Duration::from_millis(50),
        "r60: waiter 至少要等过 50ms（证明它真的在等通知，而非立即返回）"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(500),
        "r60: notify 必须在 500ms 内唤醒 waiter，实际 {:?}",
        elapsed
    );
}
