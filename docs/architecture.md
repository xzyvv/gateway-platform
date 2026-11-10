# Gateway Platform 技术架构文档

> 工业协议网关统一仓库 —— Vue 3 调试前端 + Rust 核心引擎，前后端独立编译部署，通过 REST + WebSocket 通信。

## 1. 项目概述

Gateway Platform 是面向工厂环境（表计、PLC、空调等）的工业数据采集网关，提供多协议南向采集、边缘数据处理、断网续传、多通道北向上报、配置热加载、OTA 升级等能力，并配套一套 Web 调试控制台。

| 子项目 | 语言 | 构建 | 产物 | 职责 |
|---|---|---|---|---|
| `gateway-web/` | TypeScript / Vue 3 | `npm run build` | `gateway-web/dist/` | 调试控制台（设备/点位/配置/诊断/上送测试） |
| `gateway-core/` | Rust 2021 | `cargo build --release -p gateway-core` | `target/release/libgateway_core.rlib`（引擎库） | 采集引擎 + 数据处理 + 北向上报 |
| `gateway-api/` | Rust 2021 | `cargo build --release -p gateway-api` | `target/release/libgateway_api.rlib`（Web 面库） | Web API（REST/WS，axum + tower） |
| `gatewayd/` | Rust 2021 | `cargo build --release -p gatewayd` | `target/release/gatewayd`（可执行） | 启动编排 + 网关主程序 |
| `gateway-desktop/` | Rust 2021 + Tauri 2 | `npm run build`（`tauri build`） | 桌面交付包（Tauri bundle） | 现场调试桌面客户端：内嵌后端、免浏览器直接连网关 |
| `tools/license-issuer/` | Rust 2021 | `cargo build --release -p license-issuer` | `target/release/license-issuer`（CLI） | 离线授权签发工具（Ed25519 签名，复用 `gateway-core` 的 license 格式实现，杜绝签发/校验格式漂移） |

---

## 2. 整体架构

```
                        ┌──────────────────────────────────────┐
                        │        gateway-web (Vue 3 + TS)      │
                        │  Element Plus / Pinia / Vue Router    │
                        │  Axios (REST) / WebSocket (实时)       │
                        └──────────────┬───────────────────────┘
                                       │  REST + WS (8080)
                        ┌──────────────▼───────────────────────┐
                        │        gatewayd (Rust + Tokio)        │
                        │    Web API: gateway-api / gatewayd     │
                        │  ┌──────────┐  ┌──────────────────┐  │
                        │  │ Web API  │  │  RealtimeBus     │  │
                        │  │ (axum)   │  │  (broadcast)     │  │
                        │  └────┬─────┘  └────────▲─────────┘  │
                        │       │                 │            │
                        │  ┌────▼─────────────────┴──────────┐ │
                        │  │  Pipeline (边缘过滤→入队)           │ │
                        │  └────┬─────────────────┬──────────┘ │
                        │       │                 │            │
                        │  ┌────▼─────┐    ┌──────▼─────────┐  │
                        │  │ DataQueue│    │ ConversionMgr  │  │
                        │  │ (SQLite) │    │ (北向分发)      │  │
                        │  └────┬─────┘    └──────┬─────────┘  │
                        │       │                 │            │
                        │  ┌────▼──────────────────▼─────────┐ │
                        │  │ Northbound: MQTT / Webhook /    │ │
                        │  │ OPC UA Server / Modbus          │ │
                        │  └──────────────────────────────────┘ │
                        │                                      │
                        │  ┌──────────────────────────────────┐ │
                        │  │ South: Scheduler → Collector     │ │
                        │  │ (Modbus/DLT645/CJT188/IEC104/    │ │
                        │  │  IEC101/BACnet/BACnet-MSTP/      │ │
                        │  │  S7Comm/OPC UA/MC/FINS/GPIO)     │ │
                        │  └──────────────────────────────────┘ │
                        └──────────────────────────────────────┘
```

---

## 文档目录

本文件仅保留概览与整体架构图；详细内容已按主题拆分到 `architecture/` 子目录，便于按需检索：

| 主题 | 文档 |
|---|---|
| 前端技术栈（gateway-web/） | [architecture/frontend.md](architecture/frontend.md) |
| 后端技术栈（gateway-core / gateway-api / gatewayd） | [architecture/backend.md](architecture/backend.md) |
| 通信协议（REST/WS + 南向北向） | [architecture/protocols.md](architecture/protocols.md) |
| 核心模块说明（启动流程/config/south/pipeline/queue/north/web api/platform/engine/目录约定） | [architecture/modules.md](architecture/modules.md) |
| 数据流（采集→处理→分发） | [architecture/data-flow.md](architecture/data-flow.md) |
| 部署架构（目录布局 / 进程模型 / 联调） | [architecture/deployment.md](architecture/deployment.md) |
| 关键设计约束 | [architecture/constraints.md](architecture/constraints.md) |
| API 端点清单（自动生成，CI 校验） | [api-surface.md](api-surface.md) |

> 文档总入口见 [INDEX.md](INDEX.md)。