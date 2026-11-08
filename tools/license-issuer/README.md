# license-issuer

离线授权签发工具（Ed25519 签名）。签发格式实现复用 `gateway-core` 的 `license` 模块，
与网关侧校验逻辑同源，杜绝签发/校验格式漂移。

## 这个工具在公开仓库里意味着什么

**仓库里只有签发工具，没有签发用的私钥。**

- 私钥由使用者自行生成（`keygen`），存放于 `tools/license-issuer/keys/`，
  该目录已被 `.gitignore` 排除，**私钥在任何情况下都不会进入版本库**。
- 签发格式（payload 结构、签名算法、指纹哈希口径）本身是 `gateway-core` 的公开源码，
  本仓库始终公开——**这不是本工具带来的额外暴露面**。
- 因此：拿到本仓库源码 ≠ 能签发网关认可的授权文件。签发能力取决于私钥，
  而私钥不在仓库里。

对社区使用者的实际含义：

| 场景 | 做法 |
|---|---|
| 想跑通完整功能（不受默认 100 点配额限制） | 自建钥对 → 用自己的公钥替换 `gateway-core/src/license/mod.rs` 的 `PUBLIC_KEYS` → 自行签发 `.lic` 导入设备 |
| 只想体验最小演示 | 无需任何操作，无授权即按默认配额运行（100 点 / 1 协议 / 1 通道；历史保留固定 30 天，与授权无关） |
| 想在 dev 构建里临时换公钥 | 设置环境变量 `GATEWAY_LICENSE_PUBKEY_HEX`（仅 debug 构建生效，release 构建会忽略并告警——运行时注入公钥会让自签授权绕过校验） |

## 用法

```text
license-issuer keygen [--keys <dir>]            # 生成 Ed25519 钥对（默认 ./keys）
license-issuer issue [参数]                     # 签发 .lic 文件
```

### keygen

```bash
cargo run -p license-issuer --bin license-issuer -- keygen --keys ./keys
```

产出：

- `keys/license_ed25519.pk8` —— **私钥**，PKCS#8 格式，妥善离线备份，绝不提交
- `keys/public_key.hex` —— 公钥 hex（64 字符），填入 `gateway-core/src/license/mod.rs`
  的 `PUBLIC_KEYS` 数组后随版本发布

> 私钥丢失后，已签发的授权文件仍然有效，但无法再签发新授权（需要换钥发版）。

### issue

按 **SKU 采购推荐**签发（推荐用法——不给 `--tier` / `--max-points`，自动按推荐档位填充）：

```bash
cargo run -p license-issuer --bin license-issuer -- issue \
  --license-id LIC-2026-0001 \
  --customer "某某自动化" \
  --sku sku-power \
  --machine-key <64位sha256hex> \
  --machine-parts <hash1,hash2,hash3> \
  --expires-at 2027-10-11T00:00:00Z \
  --keys ./keys \
  --out LIC-2026-0001.lic
```

手工指定各配额字段：

```bash
cargo run -p license-issuer --bin license-issuer -- issue \
  --license-id LIC-2026-0002 \
  --customer "某某自动化" \
  --tier pro \
  --max-points 2500 \
  --max-protocols 16 \
  --storage-days 30 \
  --northbound-channels 8 \
  --machine-key <64位sha256hex> \
  --machine-parts <hash1,hash2,hash3> \
  --expires-at 2027-10-11T00:00:00Z \
  --keys ./keys \
  --out LIC-2026-0002.lic
```

参数说明：

| 参数 | 说明 |
|---|---|
| `--license-id` | 唯一编号，如 `LIC-2026-0001`（必填） |
| `--customer` | 授权对象标识（必填） |
| `--sku` | 行业 SKU：`sku-minimal` / `sku-standard` / `sku-building` / `sku-power` / `sku-plc` / `full`。传入后 `--tier` 与 `--max-points` 可省略，按**采购推荐**自动填充并打印推荐说明 |
| `--tier` | `core` / `standard` / `pro` / `ultra`（与 `--sku` 二选一提供；均缺则报错） |
| `--max-points` | 最大采集点数（数字或 `none` = 无限）；不给 `--sku` 时不填则不写入 payload |
| `--max-protocols` | 最大并发协议数（同上）。v2.1 起四档默认不限 |
| `--storage-days` | 仅为 .lic 格式兼容保留（默认 30）。网关运行时历史保留固定 30 天，**不随此值变化** |
| `--northbound-channels` | 北向上行通道数（同上）。v2.1 起四档默认不限 |
| `--machine-key` | 主键指纹哈希（sha256 hex，64 字符，必填） |
| `--machine-raw` | 原始机器标识，签发机本地哈希（与 `--machine-key` 二选一） |
| `--machine-parts` | 回退项单项哈希，逗号分隔 1~3 个（可选，缺项为空串） |
| `--expires-at` | RFC3339 过期时间（可选，缺省 = 永久授权） |
| `--keys` | 私钥目录（默认 `./keys`） |
| `--out` | 输出 `.lic` 路径（默认 `<license-id>.lic`） |

四个配额参数**全部不填**时，签发的 payload 不含 `quota` 字段，网关侧按
「license 自带 quota → 配置 `tier_quotas` 查表 → 代码内置默认表 → `absent_quota` 兜底」优先级解析。

### SKU × 授权档采购推荐

SKU 是**编译期** Cargo feature 聚合（决定二进制里有哪些协议），
授权档决定**能配多少点位**——两者是两条独立的售卖轴。推荐矩阵仅供报价参考，
**不参与运行时判定**（详见
[docs/design/license-v2-quota.md](../../docs/design/license-v2-quota.md)「SKU × 授权档采购推荐」）：

| SKU | 覆盖协议 | 典型场景 | 推荐档位 | 推荐点位上限 |
|---|---|---|---|---|
| `sku-minimal` | modbus / dlt645 / cjt188 | 小规模计量 | core | 500 |
| `sku-building` | modbus / bacnet / cjt188 / opcua | 单楼宇楼控 | standard | 1500 |
| `sku-plc` | modbus / s7comm / mc / opcua | 产线 PLC 接入 | standard | 1500 |
| `sku-standard` | modbus / dlt645 / cjt188 / iec104 / opcua | 通用计量 + 远动 | pro | 2500 |
| `sku-power` | modbus / dlt645 / iec104 / iec101 / opcua | 电能量采集 + 远动 | pro | 2500 |
| `full` | 全部 9 种 | 自主集成 | ultra | 5000 |

## 私钥安全红线

- `keys/` 目录由 `.gitignore` 排除；仓库历史上从未提交过任何 `*.pk8` 或 `*.lic` 文件。
- 私钥文件仅在签发机本地存在，签发过程不写日志、不外传。
- 建议：私钥离线保存于密码管理器或离线介质，签发机上不留长期副本。

## 与网关侧的关系

| 环节 | 归属 | 说明 |
|---|---|---|
| 验签 | `gateway-core/src/license/verify.rs` | 公钥表 `PUBLIC_KEYS` 在编译期固化进二进制 |
| 机器指纹 | `gateway-core/src/license/fingerprint.rs` | 签发侧与校验侧共用同一套哈希实现（`hash_id_field` / `compute_overall`） |
| 配额解析 | `gateway-core/src/license/mod.rs`（`LicenseManager::current`） | 三级优先级见设计文档 §3 |
| 签发 | 本工具 | 只负责组装 payload + Ed25519 签名 |
