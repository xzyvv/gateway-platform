# 配置生效矩阵（热更 / 重启 / 元数据）

回答一个问题：**在前端改完并保存后，哪些参数立刻生效，哪些必须重启进程？**

判定逻辑的唯一来源是 [`gateway-core/src/config/reload_scope.rs`](../gateway-core/src/config/reload_scope.rs)；
保存链路是 `POST /api/config` → `reload_then_save`（先热重载内存，再原子落盘 + 写入配置版本库）。
本表与代码由守护用例 `every_config_section_is_classified_in_reload_scope` 对齐，新增分节必须同步登记。

## 一、保存后立即生效（热更）

| 分节 | 关键字段 | 生效方式 | 失败/边界行为 |
|---|---|---|---|
| `buses` | 端口、波特率、校验/数据位/停止位、对端地址、协议/变体、`timeout_ms`、`retry_*`、`tcp_concurrent`、`channel_capacity`、`de_pin`、`reconnect_backoff`、OPC UA 客户端认证 | 重建发生变更的总线与采集器 | 校验不过则整次保存被拒，旧配置继续运行 |
| `points` / `devices` | 点位全字段（地址、类型、字节序、公式、量程等）、设备参数 | 增量重载点位与设备 | 引用不存在设备的点位会被校验拦截 |
| `mqtt` | broker / topic / qos / 凭据 / 心跳 / 压缩 / 续传限速 / `tls.ca_cert_path` / payload 模板 | 重建 MQTT 客户端 | 新客户端连不上时**保留旧客户端**，接口返回 notes 提示 |
| `ntp` | `server`、`sync_interval` | 下一轮同步循环读取新值 | — |
| `web`（动态项） | `api_token`、`allow_anon_read`、`realtime_max_subscribers`、`webhook_test_*` | 鉴权中间件 / WS 升级流程每次请求现读 | 超并发上限的新连接直接 503 |
| `virtual_points` | 公式、单位、小数位、启用开关 | 原子替换虚拟点位规则集 | 公式非法会在保存校验时被拒 |
| `convert_rules` | 协议转换规则全字段 | 原子替换规则集 | 同上 |

## 二、保存成功但**必须重启网关**

| 分节 | 关键字段 | 为什么不能热更 |
|---|---|---|
| `web`（启动期） | `listen_addr`、`cors_origins`、`rate_limit_per_sec`、`realtime_buffer` | 监听 socket、CORS 中间件、限流器、WS 环形缓冲均在启动时构造 |
| `webhook` | 开关、URL、超时、批大小、重试、headers | 转发器在启动时一次性 spawn |
| `opcua` | server 开关、endpoint、证书、用户表、写开关 | OPC UA Server 端点只能绑定一次 |
| `modbus_server` | 开关、绑定地址、基址、模式 | 同上（监听端口） |
| `modbus_passthrough` | 开关、监听地址、目标总线、超时 | 同上（监听端口） |
| `cellular` | 全部字段（APN、拨号、心跳、failover） | 蜂窝管理器启动期初始化 |
| `telemetry` | OTLP endpoint、采样率、上报间隔 | OTel provider 只能初始化一次 |
| `self_heal` | 看门狗 7 项参数 | 看门狗线程启动期固定 |
| `queue` | `max_rows` | 淘汰线在存储层构造时固定（水位判定会按新值重算） |
| `sim_consumer` | 开关、批量、间隔、日志频率 | 启动期一次性 spawn 的通道 |

> 前端在保存成功后会弹出「需重启网关」提示，并给出 `sudo systemctl restart gateway`（容器部署重启对应容器）。

## 三、纯元数据（保存即完成，无运行时行为，不提示）

`schema_version`（内部迁移标记）、`point_trash`（回收站）、`license`（授权文件路径/配额）、
`device_groups`、`device_assets`（分组与资产，由各自页面维护）。

## 四、持久化语义

| 项 | 说明 |
|---|---|
| 配置文件 | `GATEWAY_CONFIG_PATH`（Linux 默认 `/etc/gateway/config.json`），整份原子替换 |
| 版本库 | `GATEWAY_VERSION_DB`（默认 `config/versions.db`）；`POST /api/config`、`PUT /api/config/import`、回滚、手动备份都会留快照 |
| 落盘失败 | 返回 500，但**内存已是新配置**：运行态生效、磁盘仍旧值，重启会回退（前端提示会明确说明） |
| 热重启 | 不做进程自重启（主进程无法通过 API 重启），只热更新可分节 |

## 五、新增分节时怎么做

1. 在 `GatewayConfig` 增加字段并实现 `Default`；
2. 在 `reload_scope` 中显式归类：`applied` / `restart_required` / 静默（纯元数据）；
3. 若为热更，必须在 `Gateway::reload_config` 中真正消费该字段，否则属于「静默生效」回归；
4. 同步更新本表，并确保 `every_config_section_is_classified_in_reload_scope` 通过。
