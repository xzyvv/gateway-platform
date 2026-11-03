# 新增南向协议开发指南 + 模板

> 适用场景：在网关中接入一种新的工业协议（如自定义 RS485 仪表协议、TCP PLC 协议等）。
> 本文既是分步指南，也提供**可直接复制填空的模板**（见第三节）。

## 一、协议系统架构

南向协议通过 `Collector` trait 抽象，由 `ProtocolRegistry` 统一注册。核心文件：

| 文件 | 职责 |
|------|------|
| `src/south/protocol/mod.rs` | `Collector` trait 定义 + `CollectorError` |
| `src/south/protocol/registry.rs` | 协议注册表（`PROTOCOL_ENTRIES` 单一来源 + `VALID_PROTOCOLS`） |
| `src/south/bus/mod.rs` | `BusManager`（串口/TCP 总线抽象，`transaction()` 收发） |
| `src/south/bus/frame_buffer.rs` | 串口帧缓冲区（串行协议用） |
| `src/south/protocol/skeleton/mod.rs` | 最小可运行骨架（复制改名即用） |

### 协议分类

- **串行总线（RS485）**：`modbus` / `dlt645` / `cjt188` / `bacnet_mstp`
  - 需注册 `FrameExtractor`，从 `FrameBuffer` 提取完整帧
- **TCP/UDP**：`opcua` / `iec104` / `bacnet` / `s7comm`
  - 用 `bus.tcp_addr` 作为连接端点，**不注册**帧提取器
- **点位级特殊**：`gpio`（不走 Collector 注册，仅在 `VALID_PROTOCOLS` 登记）

### Feature Gate（编译期开关）

每个协议由 Cargo feature 控制，关闭时**编译期剥离**。新增协议需在
`gateway-core/Cargo.toml` 的 `[features]` 增加对应开关，并在 `registry.rs` 的
`PROTOCOL_ENTRIES` / `VALID_PROTOCOLS` 条目上加 `#[cfg(feature = "...")]`。
详见仓库根目录 `PROTOCOL_FEATURE_GATES.md`。

## 二、开发步骤

### 步骤 1：创建协议模块

在 `src/south/protocol/` 下新建目录 `your_protocol/`，至少包含 `mod.rs`
（有帧提取器时再加 `frames.rs`）。**直接复制 `skeleton/` 目录**是最快的起点。

### 步骤 2：实现 Collector trait

必需 / 可选方法：

```rust
pub trait Collector: Send + Sync {
    fn name(&self) -> &'static str;   // 协议名，须与注册名一致
    fn bus_name(&self) -> &str;       // 所属总线端口名

    // 必需：读单点。必须手写 Pin<Box<dyn Future>>，不能用 async-trait
    // （保持对象安全，支持 Arc<dyn Collector>）
    fn read<'a>(&'a self, point: &'a PointConfig)
        -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>>;

    // 可选覆盖（有默认实现）：
    //   read_batch    批量读（默认逐点调 read）
    //   write         写操作（默认 Unsupported）
    //   scan_slaves   从站扫描（默认空）
    //   connect / disconnect / is_connected   长连接协议用
}
```

错误必须用 `CollectorError` 的**具体变体**（`NoReply` / `InvalidFrame` / `IoError` /
`InvalidConfig` …），不要用兜底变体掩盖故障类型。

### 步骤 3：实现帧提取器（仅串行协议）

在 `frames.rs` 实现 `extract_your_protocol_frame`，用 CRC/校验和确认帧完整性；
帧不完整返回 `None`（框架会等待更多字节），提取成功后 `drain` 已消费字节。

### 步骤 4：注册协议

见第三节「模板 C」：在 `PROTOCOL_ENTRIES` 追加一项（含 `capabilities`）+
底部加工厂函数 + `mod.rs` 加 `pub mod`；普通协议无需手改 `VALID_PROTOCOLS`
（由单元测试 `test_valid_protocols_match_entries` 保证同步）。

### 步骤 5：编写测试

见第四节。

## 三、可复制模板

> 把 `your_protocol` 替换为你的协议名（全小写 + 下划线，如 `s7comm` / `bacnet_mstp`）。

### 模板 A：`your_protocol/mod.rs`

```rust
//! <协议中文名> 采集器

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::config::PointConfig;
use crate::engine::metrics::Metrics;
use crate::engine::model::DataPoint;
use crate::south::bus::BusManager;
use crate::south::protocol::{Collector, CollectorError, CollectorResult};

use frames::*; // 仅串行协议需要

mod frames;      // 仅串行协议需要

pub struct YourProtocolCollector {
    port_name: String,
    bus: Arc<BusManager>,
    metrics: Arc<Metrics>,
}

impl YourProtocolCollector {
    pub fn new(port_name: &str, bus: Arc<BusManager>, metrics: Arc<Metrics>) -> Self {
        Self { port_name: port_name.to_string(), bus, metrics }
    }
}

impl Collector for YourProtocolCollector {
    fn name(&self) -> &'static str { "your_protocol" }
    fn bus_name(&self) -> &str { &self.port_name }

    fn read<'a>(
        &'a self,
        point: &'a PointConfig,
    ) -> Pin<Box<dyn Future<Output = CollectorResult<DataPoint>> + Send + 'a>> {
        Box::pin(async move {
            let req = build_request(point);
            let resp = self.bus.transaction(req, None).await?; // 串行协议
            let value = parse_response(&resp)
                .ok_or_else(|| CollectorError::InvalidFrame("parse failed".into()))?;
            let device_id = point.device_id.as_deref().unwrap_or(&point.bus);
            Ok(DataPoint::good(&point.id, device_id, value, value, "u16"))
        })
    }
}

fn build_request(point: &PointConfig) -> Vec<u8> { /* TODO: 按协议格式 */ vec![] }
fn parse_response(resp: &[u8]) -> Option<f64> { /* TODO: 按协议格式 */ None }
```

### 模板 B：`your_protocol/frames.rs`（仅串行协议）

```rust
use crate::south::bus::FrameBuffer;

/// 从缓冲区提取一帧完整报文；不足则返回 None（等待更多数据）
pub fn extract_your_protocol_frame(
    buf: &mut FrameBuffer,
    _expected_len: Option<u16>,
) -> Option<Vec<u8>> {
    let data = buf.peek();
    // TODO: 按协议起始/结束字节 + 校验（CRC/和校验）判定帧边界
    let start = data.iter().position(|&b| b == 0x68)?;
    let end = data[start..].iter().position(|&b| b == 0x16)?;
    let frame = data[start..=start + end].to_vec();
    buf.pop(start + end + 1);
    Some(frame)
}
```

### 模板 C：`registry.rs` 注册片段

```rust
// 1) 顶部 import（已带 feature gate）
#[cfg(feature = "your_protocol")]
use super::your_protocol::{extract_your_protocol_frame, YourProtocolCollector};

// 2) PROTOCOL_ENTRIES 追加一项（capabilities 尽量复用已有形态构造器）
#[cfg(feature = "your_protocol")]
ProtocolEntry {
    name: "your_protocol",
    factory: create_your_protocol,
    frame_extractor: Some(extract_your_protocol_frame), // TCP/UDP 协议填 None
    capabilities: ProtocolCapabilities::serial_bcd_meter_writable(), // 见下
},
```

`ProtocolCapabilities` 已有形态构造器（优先复用，不匹配再用字面量）：

| 构造器 | 适用形态 | batch_read | frame_extract | write | scan_slaves |
|--------|---------|:---:|:---:|:---:|:---:|
| `modbus()` | Modbus RTU/TCP | ✓ | ✓ | ✓ | ✓ |
| `serial_bcd_meter_writable()` | dlt645 / cjt188 | ✗ | ✓ | ✓ | ✗ |
| `tcp_batch_write()` | opcua / iec104 / s7comm | ✓ | ✗ | ✓ | ✗ |
| `bacnet_ip()` | BACnet/IP | ✓ | ✗ | ✓ | ✓ |
| `bacnet_mstp()` | BACnet MS/TP | ✗ | ✓ | ✗ | ✓ |

```rust
// 3) 底部加工厂函数（签名固定为 fn(port_name, bus, metrics) -> Arc<dyn Collector>）
#[cfg(feature = "your_protocol")]
fn create_your_protocol(
    port_name: &str,
    bus: Arc<BusManager>,
    metrics: Arc<Metrics>,
) -> Arc<dyn Collector> {
    Arc::new(YourProtocolCollector::new(port_name, bus, metrics))
}
```

```rust
// 4) src/south/protocol/mod.rs 追加
pub mod your_protocol;
```

### 模板 D：`gateway-core/Cargo.toml` feature

```toml
[features]
your_protocol = []   # 若依赖其它协议（如 bacnet-mstp = ["bacnet"]）在此声明
```

### 模板 E：自检清单

- [ ] `Collector` trait 必需方法全部实现，`read` 用手写 `Pin<Box<dyn Future>>`
- [ ] 串行协议实现并注册了 `FrameExtractor`
- [ ] `PROTOCOL_ENTRIES` 追加注册项并填了 `capabilities`
- [ ] `mod.rs` 加了 `pub mod`；Cargo feature 已声明
- [ ] 端到端测试覆盖真实生产路径（见第四节）
- [ ] `cargo test -p gateway-core your_protocol` 通过
- [ ] `cargo test -p gateway-core test_valid_protocols_match_entries` 通过
- [ ] `cargo clippy --lib --tests -- -D warnings` 无警告

## 四、测试要求

- **必须有真实路径的端到端用例**，不能只靠「纯函数编解码测试」或「mock 自证」。
  串行协议复用 `BusManager` 的串口 seam（`BusDeps` 注入可编程 `SerialHandle`），
  TCP/UDP 协议用真 socket 起一个最小从站桩（真协议编解码，帧构造独立于生产
  `frames.rs` 以作交叉验证 oracle）。
- 每个协议至少覆盖：**正常往返** + **异常族**（无响应/超时、坏帧/坏 CRC、握手被拒、断线重连）。
- 桩只能作为 harness，不得替代生产路径；测试须断言生产代码按协议正确反应。

## 五、配置示例

```json
{
  "buses": [
    { "port_name": "/dev/ttyS1", "protocol": "your_protocol", "baud_rate": 9600 }
  ],
  "points": [
    { "id": "p1", "bus": "/dev/ttyS1", "protocol": "your_protocol",
      "address": "1", "interval": 5, "slave_id": 1, "data_type": "u16" }
  ]
}
```

`protocol` 字段值必须与 `ProtocolEntry.name` / `VALID_PROTOCOLS` 一致；
非法值会被 `GatewayConfig::validate_all` 拒绝。
