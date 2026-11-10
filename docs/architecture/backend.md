# 后端技术栈（gateway-core / gateway-api / gatewayd）

> Gateway Platform 文档子页：原 [architecture.md](../architecture.md) 第 4 节独立成页，便于按主题检索与维护。
>
> 三个 Rust crate：引擎库 `gateway-core`（配置 / 采集 / 北向 / 队列 / 平台）、Web API 面 `gateway-api`（axum + tower，原 `gateway-core/src/web`）、可执行 `gatewayd`（启动编排 + `main` 入口）。

---

## 4. 后端技术栈（gateway-core / gateway-api / gatewayd）

### 4.1 运行时与基础

| 技术 | 版本 | 用途 |
|---|---|---|
| **Rust** | edition 2021 | 主语言，强调内存安全与零成本抽象 |
| **Tokio** | ^1 (full) | 异步运行时，驱动串口采集、网络 IO、定时调度、Web 服务 |
| **Serde / serde_json** | ^1 | 配置与 API 响应的序列化/反序列化 |
| **anyhow / thiserror** | ^1 / ^1 | 错误处理（可传播错误 + 自定义错误类型） |
| **parking_lot** | ^0.12 | 高性能锁（Mutex/RwLock） |
| **once_cell** | ^1 | 全局单例 / 延迟初始化 |
| **chrono** | ^0.4 | 时间处理 |
| **tracing / tracing-subscriber / tracing-appender** | ^0.1 / ^0.3 / ^0.2 | 结构化日志（JSON + env-filter + 文件滚动） |

### 4.2 Web 服务

| 技术 | 版本 | 用途 |
|---|---|---|
| **Axum** | ^0.7 | Web 框架（REST + WebSocket），`gateway-api/src/web/mod.rs` 注册全部路由 |
| **Tower** | ^0.4 | `ConcurrencyLimitLayer` 并发限流（64） |
| **tower-http** | ^0.5 | `CompressionLayer`(gzip)、`CorsLayer`(跨域)、`CatchPanicLayer`(防 panic 泄漏) |

### 4.3 存储

| 技术 | 版本 | 用途 |
|---|---|---|
| **rusqlite** | ^0.31 (bundled) | SQLite 数据库（WAL 模式），存历史缓冲 `data_queue.db`、MQTT outbox `mqtt_outbox.db`、配置版本 `config_versions.db` |

### 4.4 北向上报

| 技术 | 版本 | 用途 |
|---|---|---|
| **rumqttc** | ^0.24 | MQTT 客户端（rustls TLS + WebSocket），QoS1 ACK 确认 + 离线 outbox |
| **reqwest** | ^0.12 | Webhook HTTP 上报（rustls TLS，`default-features = false`） |
| **async-opcua** | ^0.19 | OPC UA Server（北向暴露点位）+ Client（南向采集） |

### 4.5 数据处理

| 技术 | 版本 | 用途 |
|---|---|---|
| **evalexpr** | ^11 | 公式表达式求值，点位公式计算（`pipeline/formula.rs`） |

### 4.6 南向采集

| 技术 | 版本 | 用途 |
|---|---|---|
| **tokio-serial** | ^5 | 串口通信，Modbus RTU / DLT645 / CJT188 / IEC101 / BACnet MSTP 总线 |
| 自研协议栈 | — | Modbus RTU/TCP/ASCII、DLT645、CJT188、IEC 101/104、BACnet/IP、BACnet MSTP、S7comm、三菱 MC、欧姆龙 FINS |

### 4.7 安全与平台

| 技术 | 版本 | 用途 |
|---|---|---|
| **ring** | ^0.17 | 密码学原语（签名、哈希） |
| **rcgen / pem / x509-parser** | ^0.13 / ^3 / ^0.16 | X.509 自签名证书生成、PEM/DER 编解码、证书字段解析 |
| **validator** | ^0.18 | 配置合法性校验 |
| **arc-swap** | ^1 | 配置热加载的原子无锁替换 |
| **rsntp** | ^3 | NTP 时间同步 |
| **gpiod** | ^0.2 (Linux only) | Linux GPIO 控制 |
| **flate2** | ^1.0 | 数据压缩 |

### 4.8 后端目录结构

```
gateway-core/src/      # 引擎库（lib）
├── config/          # 配置模型 + 加载/校验/迁移 + 模板；SQLite 运行时存储见 config/store/
│   └── cellular.rs  # 北向配置归属 config 层，避免反向依赖
├── crypto/          # 证书 / 密码 / 密钥 / base64 / sha256
├── domain/          # 中立领域常量（VALID_PROTOCOLS）
├── engine/          # 引擎基础设施：日志/指标/事件总线/健康监控/告警/审计/优雅停机
│   └── util/        # encoding.rs 二进制编解码 + model.rs 公共模型
├── license/         # Ed25519 离线验签 + 机器指纹
├── north/           # 北向：MQTT / webhook.rs / dispatch.rs（RealtimeBus 分发）/ server/（被动服务端）/ server_adapters/（数据推送适配器）
├── pipeline/        # 处理管道：边缘过滤 → 公式 → 入队 → 实时广播
├── platform/        # 平台服务：NTP / OTA / GPIO / 蜂窝 / 看门狗（含 watchdog_flush.rs 队列落盘冲刷）
├── queue/           # 断点续传队列（SQLite WAL）
├── south/           # 南向：总线管理 / 协议栈 / 调度器
│   ├── bus/         # 串口/TCP 总线 + 帧缓冲（frame_extract.rs 仅做分发，实现归各协议 frames.rs）
│   ├── protocol/    # 各协议采集器（modbus/dlt645/cjt188/iec101/iec104/bacnet/bacnet_mstp/s7comm/mc/fins/opcua/gpio）
│   │   └── common/  # 协议通用工具（decode/model/util/frame_extract）
│   └── scheduler/   # 按协议+总线分组的定时调度
└── support/         # 启动支撑：app（Gateway 组件管理器）+ paths（路径解析）

gateway-api/src/web/   # Web API 面（axum + tower，原 gateway-core/src/web）：types.rs / error.rs / middleware.rs / api/
gatewayd/src/          # bin：main.rs 入口（#[tokio::main]）+ bootstrap/ 13 步启动流程
```

---
