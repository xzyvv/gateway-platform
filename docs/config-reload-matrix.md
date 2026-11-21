# 配置生效矩阵（热更 / 重启 / 元数据）

回答一个问题：**在前端改完并保存后，哪些参数立刻生效，哪些必须重启进程？**

判定逻辑的唯一来源是 [`gateway-core/src/config/reload_scope.rs`](../gateway-core/src/config/reload_scope.rs)；
保存链路是 `POST /api/config` → `persist_then_apply`，按 **prepare（零副作用）→ 落盘 → commit（内存交换）**
三阶段执行（ROI-12），最后写入配置版本库。

- prepare 阶段校验/预构建失败（协议未注册、点位组缺 collector、新 MQTT 连不上）：
  整次保存被拒，**磁盘、内存、运行态都保持旧配置**；
- 落盘失败：同样整次不生效，什么都不改；
- commit 阶段只有串口重新打开可能失败，失败时运行态已自愈回滚，磁盘再由原始字节回滚。

因此正常情况下不会出现「磁盘已新、运行态仍旧」的中间态。本表与代码由守护用例
`every_config_section_is_classified_in_reload_scope` 对齐，新增分节必须同步登记。

## 一、保存后立即生效（热更）

| 分节 | 关键字段 | 生效方式 | 失败/边界行为 |
|---|---|---|---|
| `buses` | 端口、波特率、校验/数据位/停止位、对端地址、协议/变体、`timeout_ms`、`retry_*`、`tcp_concurrent`、`channel_capacity`、`de_pin`、`reconnect_backoff`、OPC UA 客户端认证 | 重建发生变更的总线与采集器；串口在 commit 阶段先关旧口再开新口 | TCP 新总线在 prepare 阶段建好；串口打开失败则整次保存被拒并恢复旧口，旧配置继续运行 |
| `points` / `devices` | 点位全字段（地址、类型、字节序、公式、变比、量程等）、设备参数 | 增量重载点位与设备；分组差异在 prepare 阶段算好 | 引用不存在总线/设备的点位会在 prepare 阶段被拒（整组不会静默消失）。字段语义与换算顺序见 [`点位配置与换算.md`](点位配置与换算.md) |
| `mqtt` | broker / topic / qos / 凭据 / 心跳 / 压缩 / 续传限速 / `tls.ca_cert_path` / payload 模板 | 重建 MQTT 客户端（prepare 阶段完成 ConnAck，commit 阶段原子替换） | 新客户端连不上时**整次保存被拒**，旧客户端与磁盘配置都保持不变 |
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
| 落盘失败 | 返回 500，磁盘、内存与运行态都保持旧配置（prepare 已成功但尚未 commit，预建资源自动释放） |
| 导入老配置 | `PUT /api/config/import` 遇到 `mqtt.enabled` 与 `sim_consumer.enabled` 同时为真（两个消费者分流同一批队列数据导致静默丢数）时，按存量策略**关掉模拟消费者后落盘**，响应 `sanitized` 字段给出说明——与磁盘加载路径一致，老版本备份恢复不会被 400 挡死；Web 交互式保存仍严格拒绝该组合，不让新配置继续写非法值 |
| 热重启 | 不做进程自重启（主进程无法通过 API 重启），只热更新可分节 |

## 五、新增分节时怎么做

1. 在 `GatewayConfig` 增加字段并实现 `Default`；
2. 在 `reload_scope` 中显式归类：`applied` / `restart_required` / 静默（纯元数据）；
3. 若为热更，必须在 `GatewayRuntime::prepare_reload`（预构建/校验）与 `commit_reload`（原子交换）
   中真正消费该字段，否则属于「静默生效」回归；
4. 同步更新本表，并确保 `every_config_section_is_classified_in_reload_scope` 通过。
