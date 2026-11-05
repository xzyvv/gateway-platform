# 现场 TLS 决策清单

> 部署前逐项确认，一页内得出结论。**先看三条结论**：
>
> 1. 网关内置 Web 是**明文 HTTP**，本身不提供 HTTPS；要对浏览器加密必须前置反向代理终止 TLS，证书由反代管理，**不是**网关的设备证书。
> 2. 设备自签证书 [`gateway.pem` / `gateway_key.pem`] 只服务于 **OPC UA Server** 和 **MQTT 客户端**（mTLS）。缺证书时默认自动生成，自签能通的前提是**对端按指纹信任或不做 CA 链校验**。
> 3. `GATEWAY_REQUIRE_EXTERNAL_CERT=1` 只禁用自签生成，**不会阻断启动**——配错时表现为「没有证书继续跑」，而不是启动失败。

---

## 1. 现状：每条链路怎么加密

| 链路 | 是否加密 | 证书从哪来 | 关键配置 | 代码位置 |
|---|---|---|---|---|
| Web / REST / WebSocket | 否（明文 HTTP） | 无（靠前置反代） | `web.listen_addr` | `gateway-core/src/config/web.rs:11-60` |
| MQTT 北向（`tls://` / `mqtts://`） | 是 | `mqtt.tls.ca_cert_path` 自定义 CA + 系统 CA 合并；客户端证书用全局设备证书 | `mqtt.broker` 前缀、`mqtt.tls` | `north/mqtt/broker.rs:19-34`、`north/mqtt/connection.rs:34-97` |
| MQTT 北向（`wss://`） | 是（但配置不生效） | 仅系统默认信任池 | 无 | `north/mqtt/connection.rs:93-95` |
| OPC UA Server | 有证书→`Basic256Sha256/SignAndEncrypt`；无证书→临时自签 + 明文 `None` 端点 | `opcua.cert_path/key_path` 优先，否则全局设备证书；都没有则运行时生成临时证书 | `opcua.*` | `north/server/registry.rs:117-137`、`north/server/opcua_server.rs:417-495` |
| 南向采集（Modbus / DL/T645 / CJT188 / IEC101 / FINS / BACnet） | 否 | 工业协议本身无 TLS | — | — |

设备证书文件位置与开关：

| 项 | 说明 |
|---|---|
| 目录 | `GATEWAY_CERT_DIR`；Linux 默认 `/var/lib/gateway/certs`，Windows 默认 `./certs`（`gateway-core/src/support/paths.rs:126-132`） |
| 文件 | `gateway.pem`（证书）+ `gateway_key.pem`（私钥），由 `gatewayd/src/bootstrap/helpers/north.rs:103-117` 初始化 |
| 算法 / 有效期 | ECDSA P-256 + SHA-256；`当前年-1` ~ `当前年+10`；CN `Gateway Device`；SAN 仅 `localhost` |
| 生成开关 | `GATEWAY_REQUIRE_EXTERNAL_CERT=1` 禁止自签（`crypto/cert.rs:107-113`） |
| 过期查询 | `GET /api/certificates` → `days_remaining`；`POST /api/certificates/renew` 重新生成自签 |

---

## 2. 现场必答 4 问

按顺序回答，答完即可确定配置：

1. **现场有没有 HTTPS 入口？谁终止 TLS？**
   - 有浏览器/平台直连 → 是否经过 nginx 等反代？反代证书由谁签发？网关只监听 HTTP，反代 `proxy_pass http://127.0.0.1:8080;`。
   - 无 → 内网明文访问，接受 HTTP 的可信边界。

2. **MQTT broker 用什么前缀？broker 证书谁签发？**
   - `tcp://` / `mqtt://` → 明文，不涉及证书。
   - `tls://` / `mqtts://` + 公有 CA → 只配地址即可（系统 CA 生效）。
   - `tls://` + 私有 CA / 自签 → **必须**把 broker 的 CA 证书填到 `mqtt.tls.ca_cert_path`（PEM，可含多张），否则握手失败。

3. **broker 是否要求客户端证书（mTLS）？**
   - 要求 → 网关会用全局设备证书作为客户端证书。**broker 侧必须预先导入 `gateway.pem`**，或改为让 broker 的 CA 为网关签发证书；否则 broker 拒绝连接。
   - 不要求 → 网关走单 TLS，不发送客户端证书。

4. **OPC UA 客户端怎么校验服务端？**
   - 客户端按证书指纹/手工信任 → 自签可用（把 `gateway.pem` 导入客户端信任列表）。
   - 客户端强制 CA 链校验 → 用 `opcua.cert_path/key_path` 指定由现场 CA 签发的证书（两者必须成对配置，缺文件拒绝启动）。
   - 客户端要求加密端点 → 必须提供证书；无证书时只注册明文 `None` 端点。
   - 注意默认 SAN 只有 `localhost`：用 IP/域名访问且客户端做主机名校验时会失败（见 §4 第 4 条）。

---

## 3. 按场景选配置

| 场景 | Web | MQTT | OPC UA | 结论 |
|---|---|---|---|---|
| 实验室 / 单机直连调试 | 明文 HTTP，仅 `127.0.0.1` | 明文 `tcp://` | 不开或明文 `None` | 零证书配置即可；不要设 `GATEWAY_REQUIRE_EXTERNAL_CERT` |
| 产线内网，平台只收数据 | 明文 HTTP | 明文 `tcp://` | 不开 | 零证书配置；靠网络隔离保证边界 |
| 产线内网，broker 私有 CA | 明文 HTTP | `tls://` + `ca_cert_path` | 不开 | 只配 `mqtt.tls.ca_cert_path`；自签设备证书不参与 |
| 集团平台强制 CA 链校验 + mTLS | 明文 HTTP（外侧反代） | `tls://` + `ca_cert_path`；broker 导入网关证书或其 CA 签发 | `opcua.cert_path/key_path` 指向 CA 签发证书 | `GATEWAY_REQUIRE_EXTERNAL_CERT=1` + 预置证书文件（注意 §4 第 3 条：不会阻断启动，需自检） |
| 网关 Web 对不可信网络开放 | 前置反代终止 TLS，只放行 443 | 按上 | 按上 | 网关自身仍是 HTTP；反代证书独立维护 |

---

## 4. 四个已知不一致（部署前必须知道）

1. **`mqtt.tls.insecure_skip_verify` 不生效。** 代码只在启动时打一条 `error` 日志（`north/mqtt/connection.rs:78-82`），实际仍是标准服务端证书校验。现场不要依赖它绕过自签 broker，正确做法是配 `ca_cert_path`。
2. **`wss://` 忽略 TLS 配置。** `north/mqtt/connection.rs:93-95` 走 `wss_with_default_config()`，既不读 `ca_cert_path` 也不发送客户端证书。私有 CA / mTLS 的 wss broker 会失败；需要 TLS 时优先用 `tls://`。
3. **`GATEWAY_REQUIRE_EXTERNAL_CERT=1` 不是 fail-closed 启动开关。** `get_or_create()` 返回 Err 后，`bootstrap` 只 `warn` 并以「无证书」继续（`gatewayd/src/bootstrap/helpers/north.rs:111-117`）。后果：OPC UA 退化为临时自签 + 明文 `None` 端点，MQTT 变成无客户端证书的单 TLS。上线自检必须确认 `gateway.pem` 真的存在且被加载。
4. **自签证书 SAN 只有 `localhost`。** 初始化只设置 CN，未调用 `with_san_hosts`（`bootstrap/helpers/north.rs:106-110`）。用设备 IP/域名访问且客户端做主机名校验时会握手失败；需要时改用外部证书（含正确 SAN），或让客户端按指纹信任。另：`GET /api/certificates` 的 `bound_to` 硬编码 `"mqtt,https"`（`gateway-api/src/web/api/system/handlers/certificates.rs:90`），与实际不符——该证书不用于 Web HTTPS。

---

## 5. 排障速查

| 现象 | 先看这里 | 处置 |
|---|---|---|
| `tls://` 内网 broker 连不上 | 启动日志 `mqtt: TLS enabled with system CA only` | 配 `mqtt.tls.ca_cert_path`，日志应变成 `custom CA bundle` |
| broker 拒绝网关连接（mTLS） | broker 侧日志「客户端证书未受信」 | broker 导入 `gateway.pem`，或改由 broker 的 CA 签发网关证书 |
| OPC UA 客户端报证书不受信 | 客户端信任列表 | 导入 `gateway.pem`；生产改用 CA 签发证书 |
| OPC UA 握手报安全校验失败 | endpoint 主机名 vs 证书 SAN | 用外部证书带正确 SAN，或客户端按指纹信任 |
| 浏览器 HTTPS 告警 | 反代证书，不是网关证书 | 续期反代证书；`POST /api/certificates/renew` 对 Web 无效 |
| 证书显示即将过期 | `GET /api/certificates` 的 `days_remaining` | `renew` 重生成自签（10 年）；CA 证书走 `upload` 后重启网关 |
| 启动后 OPC UA 只有 `None` 端点 | 是否有证书 | 确认 `GATEWAY_CERT_DIR` 下两文件同时存在、未被 `GATEWAY_REQUIRE_EXTERNAL_CERT` 拦住 |

相关文档：[`故障处置卡.md`](../故障处置卡.md)（卡 1 证书过期）、[`deployment.md`](deployment.md)（目录布局 / 反向代理）。
