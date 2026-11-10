# 许可证签发流程（顺序依赖）

许可证从「签发」到「现场可用」是一条**有严格先后关系**的链。顺序错了不会报错，
而是变成静默降级、或者在最后一步才失败。本文把每一条顺序依赖写清楚：
**谁必须先发生、为什么、错了会看到什么、依据在哪。**

> 面向交付/售后/供应商三方。配额模型的完整设计见
> [`design/license-v2-quota.md`](design/license-v2-quota.md)；
> 装机前的准备事项见 [`装机前准备清单.md`](装机前准备清单.md)；
> 授权失效后的现场排障见 [`故障处置卡.md`](故障处置卡.md) 卡 2。

---

## 0. 全链路一图

```
        ①公钥落地                    ②装机
   （签发用私钥，网关用公钥）      （设备码只能从本机采到）
          │                              │
          │  ③取设备码                    │
          │  GET /api/license → machine   │
          │                              │
          └──────────►  ④签发 .lic  ◄────┘
                        （私钥签名，绑定该设备码）
                               │
                               ▼
                    ⑤校准时钟（先于导入）
                               │
                               ▼
                    ⑥导入 license
              POST /api/license/import
                    或写文件 + 重启
                               │
                               ▼
                    ⑦配大批点位
              （配额生效后再加，超 100 点会被拒）
```

**一句话记法**：**先有公钥 → 先装机取码 → 签发 → 先校时 → 导入 → 再配点。**

---

## 1. 顺序依赖总表

| # | 必须先发生 | 否则 | 现场可见现象 | 依据 |
|---|---|---|---|---|
| ① | **公钥随网关版本落地** | 网关验签必失败 | `status = Invalid`，日志 `license verify failed: license signature invalid` | `gateway-core/src/license/verify.rs` `verify_file_text` |
| ② | **网关先装机** | 拿不到设备码，签发无法进行 | 无设备码可取 | `gateway-core/src/license/fingerprint.rs` `collect()` |
| ③ | **`license.path` 先配好** | 导入写盘失败 | 导入报「授权文件写入失败」，`status` 不变 | `gatewayd/.../assemble.rs` `init_license`；`gateway-core/src/license/mod.rs` `import()` |
| ④ | **系统时钟先校准** | 可能被判时钟回拨 | `status = Tampered`，日志 `license clock rollback detected` | `gateway-core/src/license/clock.rs` |
| ⑤ | **签发时决定是否传配额** | 现场改 `tier_quotas` 无效 | 改了配置但配额数字不变（`quota_source = license`） | `gateway-core/src/config/license.rs` `quota_for_tier`；`license/mod.rs` `current()` |
| ⑥ | **先导入 license，再配大批点位** | 保存配置被拒 | HTTP 4021 / 7001 / 7003 | `gateway-api/src/web/config_apply.rs` `check_config_quota` |
| ⑦ | **`config.json` 四档 `tier_quotas` 写全** | 该档失去现场可调手段 | 启动 WARN + 前端提示 `quota_source = tier_missing` | `gateway-core/src/config/license.rs` `missing_tiers()` |
| ⑧ | **（语义前提）license 失效 = 新增受限，不是断采** | — | 已配数据继续采集，新增/改配置被拒 | `design/license-v2-quota.md` §6 |

下面逐条展开。

---

## 2. ① 公钥必须先落地到网关（换钥需发版）

### 依赖内容

签发用的是**私钥**，网关验签用的是**公钥**。两者必须成对：

- 签发方私钥：`tools/license-issuer` 的 `keys/license_ed25519.pk8`（**严禁提交入仓库**，已 gitignore）
- 网关公钥：编译进二进制的 `PUBLIC_KEYS`（`gateway-core/src/license/mod.rs`）

```rust
pub const PUBLIC_KEY_OLD: [u8; 32] = [...];
pub const PUBLIC_KEYS: &[[u8; 32]] = &[PUBLIC_KEY_OLD];
```

### 为什么顺序不能反

`verify_file_text` 逐把公钥尝试 Ed25519 验签：

- 公钥表**为空** → 直接 `BadSignature`（不静默接受任何 license）
- 全部公钥都验不过 → `BadSignature` → 状态 `Invalid` → 降级

也就是说：**公钥未到位就签发，签出的 license 在网关上必然验签失败**，而且失败点在
「导入之后」，容易误判成 license 文件损坏。

### 关于 `GATEWAY_LICENSE_PUBKEY_HEX`

该环境变量可以注入额外公钥（最多 `MAX_LICENSE_PUBLIC_KEYS = 10` 把，多一把即
`BadSignature`），但**只在 debug 构建下生效**：

```rust
#[cfg(not(debug_assertions))]
{
    if std::env::var(LICENSE_PUBKEY_HEX_ENV).is_ok() {
        tracing::warn!("... is set but IGNORED in release builds ...");
    }
    return Vec::new();
}
```

原因：release 里允许运行时注入公钥 = 任何能设进程环境变量的本机使用者都能塞入
自己的公钥、自签 license，**完全绕过授权校验**。所以现场（release 二进制）
**只能靠发版换钥**，不能靠改环境变量。

### 换钥流程

1. `license-issuer keygen` 生成新钥对
2. 新公钥 hex 填进 `PUBLIC_KEYS` 数组（**追加**，旧钥保留）
3. 随版本发布

过渡期保留旧钥的意义：老客户手上的 license 仍能验签通过，直到客户升级到新版本。

### 现场自检

```bash
# release 二进制里公钥表是编译进去的，无法从文件直接读；
# 观察启动日志即可：
journalctl -u gateway | grep 'pubkey(s) loaded'
# 期望形如：1 compiled-in + 0 runtime from GATEWAY_LICENSE_PUBKEY_HEX
```

---

## 3. ② 必须先装机，才能取到设备码

### 依赖内容

设备码（机器指纹）是**从本机采集**出来的，网关没装到这台机器上就无从采集：

| 层 | 字段 | 来源 | 角色 |
|---|---|---|---|
| 主键 | `machine_key` | Linux `/etc/machine-id`（Windows `MachineGuid`） | 命中即通过 |
| 回退 1 | `onboard_mac` | 物理网卡 MAC（过滤蜂窝/虚拟/USB 网卡） | 中稳定 |
| 回退 2 | `storage_serial` | eMMC CID / NVMe serial | 出厂烧写 |
| 回退 3 | `device_serial` | devicetree serial-number / SMBIOS board_serial | 部分板缺失 |

### 为什么顺序不能反

签发工具要求 `--machine-key` 是 **64 位 sha256 hex**，且这个 hex 必须来自
**这台目标设备**。设备还没装 → 指纹还没被采集 → 拿不到码 → 签发无从开始。

### 取码方式

```bash
curl -s -H "Authorization: Bearer <api_token>" http://127.0.0.1:8080/api/license
```

返回体里的 `machine` 字段就是四层指纹（**原始值不落盘、不进 payload、不外传，
只有 sha256 哈希**）。Web 控制台的「授权管理」弹窗里有同样的内容，
每个字段旁边有「复制」按钮，还有「复制全部设备码」。

> **注意**：`/etc/machine-id` 必须是**稳定**的值。容器环境里它是随机值，
> 部署时须从宿主机挂载；否则重启一次指纹就变，授权立刻失效。

### 签发时的指纹参数对应

```bash
license-issuer issue \
  --license-id LIC-2026-0001 \
  --customer "某某自动化" \
  --tier pro \
  --machine-key  <sha256 hex，取自 machine.machine_key> \
  --machine-parts <sha256 hex>,<sha256 hex>,<sha256 hex>
```

- `--machine-key` 必须是 **64 位 sha256 hex**（不是原始 `/etc/machine-id`）。
- `--machine-parts` 最多 3 个，顺序固定为**板载 MAC → 存储序列号 → 设备序列号**，
  每项也都是 sha256 hex（取自 `machine.onboard_mac` / `machine.storage_serial` /
  `machine.device_serial`）；缺项留空即可（不足 3 个会自动补空串），
  多于 3 个直接报错。
- `--machine-raw <原始值>` 只作用于**主键**：在签发机本地哈希原始值，
  原始值不写入日志或文件。回退项没有对应的 raw 入口。
- `machine_parts` 在 payload 里**必须恰好 3 项**，否则 `validate_payload` 直接拒绝。

### 比对规则（决定「换板/换盘会不会掉授权」）

1. 主键 `machine_key` 命中 → **直接通过**
2. 否则对 3 个回退项做动态阈值比对：两侧都非空的可比对项数记为 `n`，
   匹配数 ≥ `⌈n×2/3⌉` → 通过
3. 无可比对项 → **拒绝**（fail-closed）

含义：主键稳（`machine-id` 不变）就完全不受回退项漂移影响；主键变了但板载 MAC、
存储序列号还在，通常仍能过。**换板 + 换盘**（三项全变）才会必然掉授权。

---

## 4. ③ `license.path` 必须先配好，否则导入写不进去

### 依赖内容

导入落盘写的是 manager 记录的**授权路径**，而这个路径在启动时就已解析定下：

```
环境变量 GATEWAY_LICENSE_PATH  >  config.license.path  >  默认 ./license.lic
```

（`gatewayd/src/bootstrap/helpers/assemble.rs` 的 `init_license`）

### 为什么顺序不能反

- 默认的 `./license.lic` 是**相对进程 CWD** 的，而 unit 的
  `WorkingDirectory=/opt/gatewayd`；
- `gateway.service` 有 `ProtectSystem=strict`，`ReadWritePaths=` 只列了
  `/var/lib/gateway /var/log/gateway /tmp /etc/gateway` ——
  **`/opt/gatewayd` 不在其中，是只读的**；
- 于是 `std::fs::write` 失败 → `LicenseError::Io` → 导入返回
  「授权文件写入失败：请检查网关工作目录的写入权限」。

### 正确做法

`config.json` 里配绝对路径（出厂模板已配好）：

```json
"license": { "path": "/etc/gateway/license.lic" }
```

`/etc/gateway` 在 `ReadWritePaths` 内 → 可写。

> **注意**：路径在**启动时**解析。若是导入失败后才改的 `config.json`，
> **必须重启服务**让 manager 重新记住路径，否则 `import()` 仍会往旧路径写
> （manager 路径为 `None` 时会退化成 `license.lic` 相对路径 —— 同样落在只读的
> `/opt/gatewayd`）。

---

## 5. ④ 必须先校准时钟，再导入

### 依赖内容

过期判定用系统时钟；同时有一套「时钟回拨防护」会**持久化已见过的最大时间戳**。

### 机制

- 基准存**双副本**：SQLite 主副本 `/var/lib/gateway/license_clock.db`
  + 旁路文件 `<db>.replica`（格式 `GWCK2 <last_seen> <writes> <hmac_hex>`，
  HMAC 用机器绑定密钥）
- 判定：
  - `now < last_seen - 48h`（`ROLLBACK_THRESHOLD_SECS`）→ `Tampered`
  - 仅旁路副本幸存（主库被删）→ **直接判 `Tampered`**（标准攻击路径）
  - 双副本全丢时，与 license 文件 mtime 交叉校验：当前时间比 license 文件的 mtime
    **早超过 48h**（= license 文件落在「未来」）→ `Tampered`
  - 副本读失败 → **fail-closed**（判时钟不可信 → 降级）
- NTP 校时成功后调 `ClockGuard::resync` **重置**基准（无需重启）

### 为什么顺序不能反

无 RTC 电池的工控板断电后时间漂回过去是常态。**如果先导入、后校时**：

1. 导入时校验走一遍 `check_and_update`，把**错误的时间**写成了基准；
2. 之后即使校时成功，`resync` 会重置基准 → 正常情况下能恢复；
3. 但若在两次之间服务重启、或副本机制出现缺失，就会被判 `Tampered`。

**先校时再导入**，等于让第一次基准就是真实时间，后续一切正常。

### 正确顺序

```bash
# 1. 先确认/校准时钟
curl -s -X POST -H "Authorization: Bearer <token>" http://127.0.0.1:8080/api/ntp/sync
curl -s -H "Authorization: Bearer <token>" http://127.0.0.1:8080/api/ntp/status

# 2. 再导入 license
```

网关侧有 60s 周期的对账任务：发现新的成功 NTP 校时就自动 `apply_ntp_resync`
并立即重校验。所以「校时后授权自己恢复了」是预期行为，不用重启。

### 过期与临期

- `expires_at` 为 `null` → **永久授权**（签发时不传 `--expires-at` 即为此）
- 剩余 ≤ 30 天（`EXPIRING_WINDOW_SECS`）→ `status = Expiring`，**功能不受限**
- 已过期 → `status = Expired` → 降级（采集不停，新增受限）

> 导入路径上的 `dry_parse` **只做验签 + 指纹比对，不查有效期**。
> 所以过期文件**能导入成功**，导入后状态由 `current()` 呈现为降级态 ——
> 这是有意设计：文件本身是供应商签发的合法产物。

---

## 6. ⑤ 签发时是否传配额参数，决定现场还能不能调配额

### 依赖内容

配额有 4 级来源，**优先级从高到低**：

```
① license 文件自带 quota        （quota_source = license）
② config.json 的 license.tier_quotas 查表命中（quota_source = tier_config）
③ 代码内置四档默认表 DEFAULT_TIER_QUOTAS（quota_source = tier_missing）
④ license 缺失/失效 → absent_quota（quota_source = absent_fallback）
```

**注意**：查表（②）**只在 license 有效时发生**。无 license 或授权失效一律走
`absent` 兜底（`DEFAULT_ABSENT_QUOTA` = 100 点 / 1 协议 / 30 天 / 1 通道）。

### 为什么顺序不能反

`license-issuer issue` 的配额参数是**可选**的：

- **传了** `--max-points`（或 `--sku` 自动带出）→ quota 写进 payload →
  现场调整 `config.json` 的 `tier_quotas` **不会生效**（① 优先于 ②）
- **不传** → payload 里 `quota = None` → 现场改 `config.json` 的 `tier_quotas`
  **立即生效**，不用重新签发、不用换版本

### 签发方的选择

| 场景 | 建议 |
|---|---|
| 配额要跟合同绑定，现场不许自己调 | 显式传 `--max-points` 等参数（quota 写死在 license 里） |
| 想保留现场按实际规模调整的能力 | **不传**配额参数，只签 `--tier`；现场改 `config.json` 的 `tier_quotas` 即可 |
| 用 `--sku` 按行业采购推荐 | `--sku` 会**自动带出**推荐档位与推荐点数（`quota` 被写入 payload）——**要留现场调整余地就不能用 `--sku` 自动填充** |

`--max-points none` = 无限；其余三项（`--max-protocols` / `--storage-days` /
`--northbound-channels`）同理。

### v2.1 口径提醒

配额**只管 `max_points`**（采集点数）。`max_protocols` / `northbound_channels`
自 v2.1 起默认不限、且**不再作为拒绝保存配置的依据**（只告警）；
`storage_days` 自 v2.1 起与授权无关（运行时固定 30 天）。四档默认表：

| 档位 | `max_points` |
|---|---|
| core | 500 |
| standard | 1500 |
| pro | 2500 |
| ultra | 5000 |

---

## 7. ⑥ 先导入 license，再配大批点位

### 依赖内容

无 license 时配额只有 **100 点**（`DEFAULT_ABSENT_QUOTA`）。

### 为什么顺序不能反

保存配置时 `check_config_quota` 只对**采集点数**做硬性拒绝。点位超限的错误码
按**授权状态**翻译（不是一律 4021）：

| 授权状态 | 错误码 | 含义 |
|---|---|---|
| 有效但档位配额不够 | **4021** | 升档位或精简配置 |
| 从未导入 license | **7001** | 先导入授权 |
| 授权已失效（过期/指纹不符/验签失败） | **7003** | 先处理授权失效 |

所以：**先配 500 个点位再导入 license**，中途每保存一次都会被拒；
而**先导入 license 再配点位**，全程可正常保存。

### 一个反向场景

导入**低档位** license 时，如果存量配置已超出新档位配额：

- **导入不会被阻断**（license 已经生效，配额是既定事实）
- 但会打 WARN 并写审计（`license.quota_exceeded`）
- 用户要等到**下次保存配置**才会收到 4021

也就是说：低档位 license 导入后，**存量配置可能已经超限**，但界面不报错。

---

## 8. ⑦ 四档 `tier_quotas` 建议在 `config.json` 里显式写全

### 依赖内容

出厂模板 `config.factory.json` 已经把四档写全了（core 500 / standard 1500 /
pro 2500 / ultra 5000，`max_protocols` 与 `northbound_channels` 为 `null`=不限，
`storage_days` 30）。

### 漏配的后果

代码里有内置默认表 `DEFAULT_TIER_QUOTAS` 兜底，所以**漏配不会掉到开源版配额**。
但：

- 运维**失去**按现场调整该档配额的手段（改 `config.json` 也读不到）
- 启动时打 WARN，前端提示 `quota_source = tier_missing`（提示「配置缺该档位，配额已降级」）

对 ④ 档位没配但又要留现场调整余地的场景，**显式写全四档**。

---

## 9. ⑧ 语义前提：license 失效 = 新增受限，不是断采

四种失效态（`Expired` / `Mismatch` / `Tampered` / `Invalid`）对外行为**完全一致**：

```
present = false，tier = core，配额走 absent 兜底
```

- **已配置的数据继续采集、继续落库**（原本 200 点，失效后仍继续采集，不会突然停止）
- **任何新增/修改配置操作被拒**（4021 / 7001 / 7003）
- 区分只体现在 `status` 上，便于排查

现场结论：**授权失效不会中断采集，但无法新增点位。**

---

## 10. 签发方操作清单（按序执行）

```bash
# 0. 一次性：生成钥对（私钥永不入库，需离线备份）
license-issuer keygen --keys ./keys
# → 把 keys/public_key.hex 的内容填进 gateway-core/src/license/mod.rs 的 PUBLIC_KEYS
# → 发版（公钥必须随二进制落地，见 §2）

# 1. 装机（取设备码的前提）
sudo bash deliver.sh

# 2. 取设备码（从目标网关上取）
curl -s -H "Authorization: Bearer <token>" http://127.0.0.1:8080/api/license | jq .machine

# 3. 签发（不传配额参数 → 保留现场调整余地）
license-issuer issue \
  --license-id LIC-2026-0001 \
  --customer "某某自动化" \
  --tier pro \
  --machine-key <sha256 hex> \
  --machine-parts <mac>,<storage>,<device> \
  --out LIC-2026-0001.lic

# 4. 现场先校时，再导入
curl -s -X POST -H "Authorization: Bearer <token>" http://127.0.0.1:8080/api/ntp/sync
curl -s -X POST -H "Authorization: Bearer <token>" \
     -H "Content-Type: application/json" \
     -d '{"content":"<base64(payload).base64(sig)>"}' \
     http://127.0.0.1:8080/api/license/import

# 5. 确认
curl -s -H "Authorization: Bearer <token>" http://127.0.0.1:8080/api/license
# → status = Ok，present = true，tier = Pro，quota_source 按 §6 判定
```

---

## 11. 现场排查对照表

| 现象 | 最可能的原因 | 处置 |
|---|---|---|
| 导入报「授权文件签名无效」 | 网关二进制里没有对应公钥（§2） | 确认公钥已编译进该版本并已发版到现场 |
| 导入报「授权文件与本设备不匹配」 | 设备码取自另一台机器，或 `/etc/machine-id` 变了（§3） | 重新取码签发，**不要手工改指纹** |
| 导入报「授权文件写入失败」 | `license.path` 指向只读目录（§4） | 改成 `/etc/gateway/license.lic` 并**重启** |
| 导入成功但 `status = Tampered` | 时钟回拨（§5） | 先校时；NTP 校时成功后自动重置，无需重启 |
| 导入成功但 `status = Expired` | 签发时没传 `--expires-at` → 永久；或已过期 | 向供应商申请续期 |
| 配额数字改了不生效 | license 自带 quota 优先（§6） | 要保留现场可调能力，需**不传**配额参数重新签发 |
| `quota_source = tier_missing` | `config.json` 漏配该档（§8） | 显式写全四档 |
| 加点位被拒 4021 / 7001 / 7003 | 配额不足 / 未导入 / 授权失效（§7、§9） | 按错误码对应处理 |
| 授权「自动恢复」 | NTP 校时后 `apply_ntp_resync` 自动重校验（§5） | 预期行为，不用重启 |

---

## 12. 相关文档

- [`装机前准备清单.md`](装机前准备清单.md)——现场装机前要准备的东西（含 §4.3 许可证路径）
- [`故障处置卡.md`](故障处置卡.md) 卡 2 / 卡 3——License 到期与 NTP 失步的现场处置
- [`design/license-v2-quota.md`](design/license-v2-quota.md)——配额模型完整设计（现行 v2.0/v2.1）
- [`../tools/license-issuer/README.md`](../tools/license-issuer/README.md)——签发工具用法与私钥红线
- [`api-surface.md`](api-surface.md)——`/api/license` / `/api/license/import` / `/api/ntp/*` 端点契约
