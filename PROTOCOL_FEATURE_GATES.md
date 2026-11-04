# 协议插件化 Feature Gate 使用说明

## 概述

gateway-core 支持编译期按需选择协议，关闭的协议在编译期完全剥离，零运行时成本。
默认启用标准发行集（`default = ["all-protocols"]`，9 个协议），不破坏现有 `cargo build` 行为。
FINS 与 BACnet MS/TP 已移出默认集合并进入「退役观察」：feature 仍保留，仅用于存量
项目或回归测试显式启用，连续两个版本无项目需求后删除。

## 可用协议 Feature

| Feature 名 | 协议 | 类型 | 依赖 | 发行状态 |
|------------|------|------|------|----------|
| `modbus` | Modbus RTU/TCP | 串行 + TCP | — | 默认 / 全部 SKU |
| `dlt645` | DL/T 645 电力仪表 | 串行 (RS485) | — | 默认 / 标准版 / 电力版 / 精简版 |
| `cjt188` | CJ/T 188 水气热仪表 | 串行 (RS485) | — | 默认 / 标准版 / 楼宇版 / 精简版 |
| `opcua` | OPC UA **双向合并别名**（= `opcua-client` + `opcua-server`） | TCP | — | 默认 / 标准版 / 楼宇版 / 电力版 / PLC 版 |
| `opcua-client` | OPC UA **南向客户端**（采集外部 OPC UA 服务端） | TCP | — | 需要采集 UA 设备时 |
| `opcua-server` | OPC UA **北向服务端**（把点位暴露给 SCADA） | TCP | — | 需要给 SCADA/组态看数时 |
| `iec104` | IEC 60870-5-104 | TCP | — | 默认 / 标准版 / 电力版 |
| `bacnet` | BACnet/IP | UDP | — | 楼宇版 |
| `s7comm` | S7comm 西门子 PLC | TCP | — | PLC 版 |
| `mc` | 三菱 MC（Melsec，QnA 3E 二进制帧） | TCP | — | PLC 版 |
| `iec101` | IEC 60870-5-101（串行链路 FT1.2） | 串行 | — | 电力版 |
| `fins` | 欧姆龙 FINS（FINS/TCP） | TCP | — | **退役观察** |
| `bacnet-mstp` | BACnet MS/TP | 串行 (RS485) | `bacnet` | **退役观察** |
| `all-protocols` | 聚合：默认发行集 9 个协议 | — | 上述除退役项 | 默认 |
| `sku-standard` | 行业 SKU：标准版 | — | modbus/dlt645/cjt188/iec104/opcua | 发行默认 |
| `sku-building` | 行业 SKU：楼宇版 | — | modbus/bacnet/cjt188/opcua | 行业发行 |
| `sku-power` | 行业 SKU：电力版 | — | modbus/dlt645/iec104/iec101/opcua | 行业发行 |
| `sku-plc` | 行业 SKU：PLC 版 | — | modbus/s7comm/mc/opcua | 行业发行 |
| `sku-minimal` | 行业 SKU：精简版 | — | modbus/dlt645/cjt188 | 行业发行 |

> `gpio` 是点位级协议，不走 Collector 注册流程，始终编译，不受 feature gate 控制，随所有 SKU 交付。

**OPC UA 为什么拆两向**：南向 Client（`south/protocol/opcua`）与北向 Server
（`north/server/opcua_server` + `north/server_sinks/opcua_sink`）客户群几乎不重叠，
且分别只拉 `async-opcua` 的不同子 crate（client → `async-opcua-client`；
server → `async-opcua-server`、`async-opcua-nodes`、`async-opcua-core-namespace`）。
此前 `async-opcua` 是非 optional 硬依赖、北向 Server 又无 `cfg` 门控，
导致 5 个 SKU 依赖树完全相同（335 crate）、体积只差 ≤11 KB；
拆分后 `sku-minimal` 依赖树降到 288 crate，不再链接任何 OPC UA 代码。

> 未编译 `opcua-server` 的构建若在配置里启用了北向 OPC UA Server，启动时会打
> `warn`（而不是静默不启动）——现场据此判断「是二进制没带能力」还是「网络问题」。

## 非协议 Feature

以下 feature 与协议无关，但同样采用编译期门控，默认关闭。

| Feature 名 | 作用 | 归属 crate | 关闭时的行为 |
|------------|------|-----------|--------------|
| `encryption` | SQLite 加密（SQLCipher） | `gateway-core` | 落盘数据库不加密 |
| `tracing` | 数据管道 `#[instrument]` / `info_span!` 埋点 | `gateway-core` | 不展开埋点宏 |
| `tracing-otlp` | OTLP 远程 trace 导出（→ Jaeger/Tempo/Grafana），隐含 `tracing` | `gateway-core` | 不编译 opentelemetry 栈 |
| `remote-shell` | 远程诊断只读 Shell（`/api/diagnostics/exec`） | `gateway-core` → `gateway-api` → `gatewayd` 链式转发 | 路由与鉴权保留，返回 400 `remote shell is disabled in this build (rebuild with --features remote-shell)` |
| `test-support` | 测试签发辅助：`license::issuer_support`（一次性 Ed25519 钥对生成，内含 clippy 豁免的 `unwrap`） | `gateway-core`（`gateway-api` 的 dev-dependencies 已自动启用） | 模块不编译；`gatewayd` / `gateway-desktop` 等发行二进制不含该代码，panic 面不出现在生产构建 |

`remote-shell` 的特殊之处：它跨越三个 crate，通过 Cargo feature 依赖链逐层转发，
关闭时**整条 Shell 执行链路（含白名单校验、输出截断等加固代码）在编译期完全剥离**，
但日志 tail / 归档下载等诊断能力不受影响。

## 常用构建命令

### 默认发行集构建（不破坏现有行为）

```bash
cargo build -p gateway-core
cargo test -p gateway-core
```

### 行业 SKU 构建

各 SKU 按南向协议互斥切片，只装本行业需要的协议：

```bash
# 标准版（发行默认）：modbus/dlt645/cjt188/iec104/opcua
cargo build -p gatewayd --no-default-features --features sku-standard

# 楼宇版：modbus/bacnet/cjt188/opcua（不含 MS/TP）
cargo build -p gatewayd --no-default-features --features sku-building

# 电力版：modbus/dlt645/iec104/iec101/opcua
cargo build -p gatewayd --no-default-features --features sku-power

# PLC 版：modbus/s7comm/mc/opcua
cargo build -p gatewayd --no-default-features --features sku-plc

# 精简版：modbus/dlt645/cjt188
cargo build -p gatewayd --no-default-features --features sku-minimal
```

### 最小构建（仅指定协议）

```bash
# 水厂场景：Modbus + CJT188
cargo build -p gateway-core --no-default-features --features modbus,cjt188

# 变电站场景：DL/T 645 + IEC 104
cargo build -p gateway-core --no-default-features --features dlt645,iec104

# 存量楼宇总线（退役观察，仅显式启用）：BACnet MS/TP（自动带入 bacnet）
cargo build -p gateway-core --no-default-features --features bacnet-mstp

# 西门子产线：S7comm + Modbus
cargo build -p gateway-core --no-default-features --features s7comm,modbus
```

### 单协议测试

```bash
cargo test -p gateway-core --no-default-features --features modbus
```

### 与其他 feature 组合

协议 feature 可与 `tracing`、`encryption`、`remote-shell` 等自由组合：

```bash
# 最小体积 + 链路追踪
cargo build -p gateway-core --no-default-features --features modbus,tracing

# 全量协议 + SQLite 加密
cargo build -p gateway-core --features encryption

# 现场排障需要执行只读 Shell 命令时（跨三个 crate 链式启用）
cargo build -p gatewayd --features remote-shell
```

## 配置校验

`VALID_PROTOCOLS` 是配置校验的唯一来源，自动只含启用的协议。
若 config.json 引用了未启用的协议，启动时区分两种错误：

- 协议拼错：`unknown protocol '<name>', valid protocols in this build: [...]`
- 协议存在但被当前 SKU 裁剪 / 处于退役观察：

```
协议 'fins' 未包含在当前发行版（已按 SKU 裁剪或处于退役观察）。
本版可用协议: ["modbus", ...]；如确需该协议，请重新构建并启用 feature 'fins'，
或改用包含它的发行包。
```

两者都由 `domain::protocol_error_message` 统一生成，`validate_protocol` 与运行时
`ProtocolRegistry::create` 返回 None 的路径共用同一文案。

## 新增协议时的影响

新增协议时需同步修改以下文件，并为所有新增项加 `#[cfg(feature = "your_protocol")]`：

1. **`gateway-core/Cargo.toml`** — `[features]` 新增 `your_protocol = []`，并加入 `all-protocols` 列表
2. **`protocol/mod.rs`** — `#[cfg(feature = "your_protocol")] pub mod your_protocol;`
3. **`protocol/registry.rs`** — 两处加 `#[cfg]`：
   - 工厂函数 `create_your_protocol` 与相关 `use super::your_protocol::...`
   - `PROTOCOL_ENTRIES` 中的 `ProtocolEntry`
4. **`domain.rs`** — `VALID_PROTOCOLS` 数组追加协议名并加 `#[cfg]`
   - 该数组定义在 `gateway-core/src/domain.rs`（供 `config` 与 `south` 共用，避免反向依赖），
     `registry.rs` 只是 `pub use crate::domain::VALID_PROTOCOLS;`，**不会自动同步**
   - 例外：`gpio` 为点位级协议，始终合法、不加 feature 门控

一致性由 `south::protocol::registry` 的单元测试自动校验：
`test_valid_protocols_match_entries` 与 `test_protocol_three_way_sync`
（前者校验 `VALID_PROTOCOLS` ↔ `PROTOCOL_ENTRIES` 映射，后者额外校验
`frame_extractor` 标记与 `ProtocolRegistry` 查找结果一致）。

> `skeleton` 协议模板只在测试构建下编译（`protocol/mod.rs` 内为 `#[cfg(test)] pub mod skeleton;`），
> 生产构建不包含它；复制模板新增真实协议时，记得把 `#[cfg(test)]` 换成真实 feature 门控。

## 设计原理

- **编译期剥离**：`#[cfg(feature = "...")]` 在编译期展开，关闭的协议模块、注册项、工厂函数不进二进制
- **零运行时成本**：与运行时 if/else 不同，关闭的代码完全不存在于最终二进制中
- **默认发行集**：`default = ["all-protocols"]`（9 协议）保证现有工作流不受影响；
  行业发行由 `sku-*` 聚合 feature 显式裁剪
- **跨模块依赖处理**：
  - `bacnet-mstp` 依赖 `bacnet`（Cargo feature 依赖声明）
  - `bus/tcp_task.rs` 依赖 `modbus`（Modbus TCP 总线任务，feature gate 控制）
  - `CollectorError::ModbusException` 变体在 modbus 关闭时消失

## 部署建议

### 出厂镜像：用构建脚本显式传协议清单

```bash
# 出厂默认：标准版 SKU
./deploy/build-release.sh

# 行业 SKU（含网关前端 + gatewayd 一起构建）
./deploy/build-release.sh --sku building
./deploy/build-release.sh --sku plc
./deploy/build-release.sh --sku minimal

# 交叉编译到 ARM64
./deploy/build-release.sh --target linux-arm64 --sku power

# 自定义子集
./deploy/build-release.sh --protocols modbus,cjt188

# 默认发行集全量（仅体积基准/自测，不做裁剪）
./deploy/build-release.sh --protocols all
```

脚本会做四件事：

1. **SKU 解析**：`--sku standard|building|power|plc|minimal`（默认 `standard`），换算成
   `--no-default-features --features sku-<name>`；`--protocols` 可显式覆盖 SKU。
   SKU → 协议清单不在脚本里重复维护，运行时直接解析 `gateway-core/Cargo.toml` 的
   `sku-*` 聚合 feature（单一来源）；
2. **白名单校验**：协议名写错时提前报错并列出可用 feature，不让 cargo 抛不可读的编译错误；
3. **强制裁剪**：非 `all` 时一律追加 `--no-default-features`，避免
   `default = ["all-protocols"]` 把默认发行集无条件编进来；
4. **写 `deploy/out/BUILD-INFO.txt`**：记录 `build_time / target / sku / protocols / feature_args`，
   作为发布检查依据（随产物一起分发）。

### 发布检查项：协议裁剪一致性

```bash
# 在目标机上部署完成后执行，默认读取 deploy/out/BUILD-INFO.txt
sudo bash deploy/verify-deploy.sh --token <api_token>
# 构建信息不在默认位置时
sudo bash deploy/verify-deploy.sh --token <api_token> --build-info /tmp/gw-out/BUILD-INFO.txt
```

校验逻辑：取 `BUILD-INFO.txt` 的 `protocols=` 与 `GET /api/protocols` 返回的协议名比对
（`bacnet-mstp` → `bacnet_mstp` 归一化，`gpio` 始终编译、不参与比对）。
**不一致即 FAIL**：多为发错包（用了全量构建或旧产物），也可能是清单与 `Cargo.toml` feature 名不同步。
若线上拿不到 API 或文件缺失，脚本降级为 WARN 跳过，不阻断其它检查。

| 部署场景 | 推荐构建参数 | 包含协议 |
|----------|-------------|---------|
| 标准版（默认） | `./deploy/build-release.sh` 或 `--sku standard` | modbus/dlt645/cjt188/iec104/opcua + gpio |
| 楼宇版 | `--sku building` | modbus/bacnet/cjt188/opcua + gpio |
| 电力版 | `--sku power` | modbus/dlt645/iec104/iec101/opcua + gpio |
| PLC 版 | `--sku plc` | modbus/s7comm/mc/opcua + gpio |
| 精简版 | `--sku minimal` | modbus/dlt645/cjt188 + gpio |
| 水务/计量项目子集 | `--protocols modbus,cjt188,dlt645` | 按清单 |
| 存量 FINS 项目（退役观察） | `--protocols modbus,dlt645,cjt188,iec104,s7comm,mc,opcua,fins` | 显式清单 |
| 存量 MS/TP 项目（退役观察） | `--protocols bacnet-mstp` | BACnet/IP + MS/TP |
| 体积基准 | `--protocols all` | 默认发行集 9 协议 |

> 瘦身收益必须显式指定 SKU 或协议清单才能拿到：默认 `cargo build` 会启用 `all-protocols`，出厂镜像若直接使用默认构建则体积为默认发行集基准，不产生行业裁剪收益。
