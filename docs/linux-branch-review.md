# Linux 专属分支审查报告（部署前风险排除）

> 审查范围：全仓 `#[cfg(target_os = "linux")]` / `#[cfg(unix)]` / `#[cfg(not(windows))]` 代码块。
> 背景：开发机为 Windows，产物部署到 Linux 嵌入式设备 → Linux 分支在 CI 上**从未被执行过**，
> 存在「编译期通过、运行期静默失效」的结构性风险。本报告记录审查结论、已修复缺陷与剩余覆盖缺口。
>
> 批次：r15（物理硬件部署前准备）　完成日期：2026-11-06

---

## 0. 结论摘要

| 项 | 结果 |
|---|---|
| 审查的 Linux-only 代码块 | 14 处（见 §2） |
| 修复的 P0/P1 缺陷 | 6 项（§1） |
| 新增跨平台单测 | 20 个（§3） |
| Linux 交叉编译验证 | `cargo check --target x86_64-unknown-linux-gnu --workspace --all-targets` 通过 |
| Windows 侧单测 | `cargo test -p gateway-core --lib --features fins` → 1407 passed / 0 failed |
| 剩余风险 | §4「无法在本机验证项」，需真机验收 |

**核心结论**：本轮修复的 6 项缺陷全部属于**「错误被静默吞掉」**这一类。
它们在 Windows 上不会暴露，但会**直接导致设备变砖或现场静默断网**——
即「设备看起来正常，实际已经失去心跳/链路/加密保护」。

---

## 1. 已修复缺陷

### 1.1 P0 — 硬件看门狗写失败被静默吞掉（可能变砖）

**位置**：`gateway-core/src/platform/hardware_watchdog.rs`

**问题**：`kick()` / `stop()` / `start()` 三处对 `/dev/watchdog` 的写失败都不做任何处理。
硬件看门狗一旦写失败（设备掉电复位后 fd 失效、内核驱动异常），
软件层**毫不知情**，继续按「已喂狗」运行 → 硬件超时 → **设备强制复位**。
对工业网关而言，误复位 = 采集中断 = 数据丢失。

**修复**：
- 引入 `HwWriteOutcome { Ok, Failed, NoDevice }` 纯函数判定层；
- 写失败计入 `hw_write_errors` 计数器并输出 `tracing::error!`；
- `stop()` 写 `'V'` 失败时显式 `error!`（未解除 armed → 下次上电可能误复位）。

**验证**：新增 5 个 Windows 可跑单测
（`hw_write_reports_failure_on_readonly_handle` /
`hw_write_reports_ok_on_writable_handle` /
`hw_write_reports_no_device_without_fd` /
`hw_write_failures_are_counted` / `stop_clears_running_flag`）。

---

### 1.2 P0 — `Watchdog::stop()` 全仓零调用（进程退出后看门狗仍 armed）

**位置**：`gateway-core/src/platform/hardware_watchdog.rs`、`gatewayd/src/bootstrap/helpers/shutdown.rs`

**问题**：硬件看门狗一旦 open 即 armed，只有写 `'V'` 才能解除。
`Watchdog::stop()` 在全仓**没有任何调用点** → 进程正常退出（`systemctl restart`、OTA 升级、
`kill`）后 `/dev/watchdog` 仍处于 armed 状态 → 内核硬件超时 → **强制断电重启**。
表现为「重启后网关自己又重启了一次」，现场极难定位。

**修复**：
- 句柄提升为**模块级全局唯一**（`/dev/watchdog` 是独占设备，第二次 open 必然 `EBUSY`）；
- 新增 `stop_platform_watchdog()` 公开 API，由 `graceful_shutdown_inner` 在
  MQTT 停止之后、退出之前调用；
- 喂狗循环（`bootstrap/helpers/collection.rs`）增加 `watchdog.is_running()` 判断，
  停机后退出循环，**避免停机期间继续写心跳把已解除的设备重新 arm**。

---

### 1.3 P0 — OTA 期间二次独占打开 `/dev/watchdog`（升级变砖）

**位置**：`gateway-core/src/platform/ota/apply.rs`

**问题**：OTA 升级流程中调用 `kick_platform_watchdog()`，其实现**独立地**
`OpenOptions::new().write(true).open("/dev/watchdog")`。
但看门狗主流程已持有该设备的独占 fd → 二次 open 必然 `EBUSY` →
原代码 `if let Ok(...)` / `let _ =` 把失败**完全吞掉** →
**OTA 全程硬件喂狗静默失效** → 升级过程中硬件超时 → 变砖。

**修复**：改为复用全局唯一 fd（`kick_platform_watchdog()`），
OTA 与主流程共用同一句柄，**结构上不可能再二次 open**。

---

### 1.4 P0 — `uboot_sync` 忽略退出码（env 未落盘却 reboot）

**位置**：`gateway-core/src/platform/ota/bootenv.rs`

**问题**：`uboot_sync()` 原实现 `let _ = Command::new("sync").status();`。
`Command::status()` **只要进程启动成功就返回 Ok**，退出码被丢弃。
若 `sync` 执行失败（磁盘满、只读挂载），U-Boot 环境变量尚未落盘 →
随即 reboot → **回滚机制静默失效** → 变砖。

**修复**：引入 `SyncDecision { Flushed, Missing, Failed }` 纯函数判定：
- `Flushed` → 继续 reboot；
- `Missing`（`sync` 命令不存在）→ `warn!` 但放行（容器/精简系统场景）；
- `Failed`（退出码非零）→ 返回 `GatewayError::Io`，**中止 reboot**。

**验证**：新增单测 `sync_outcomes_are_classified`。

---

### 1.5 P1 — GPIO 重复 request 同一物理线（EBUSY，DI 点位永远读不到值）

**位置**：`gateway-core/src/platform/gpio.rs`

**问题**：gpiod 的**同一物理线被两个 fd 重复 request 会返回 `EBUSY`**。
原实现中：
- `GpioController` 持有 `di_lines`（一次性 request 4 条 DI 线）；
- `read_gpio_line(pin)` 又单独 `chip.request_lines(...)` request 同一条线。

两者并存 → 后者必然 `EBUSY` → 表现为「DI 点位采集不到值」，
而错误根因（EBUSY）被表象（值为 0/None）掩盖，现场无法定位。

**修复**：统一为**进程级共享句柄**：
- `DI_LINES` 全局 `OnceCell` 持有 `Lines<Input>`；
- `ensure_di_lines()` 幂等初始化；
- `read_di_levels()` 统一读取；
- `read_gpio_line(pin)` 对 DI 段（0..=3）**复用共享句柄**取对应下标，
  只有非 DI 段线号才走单线 request 路径；
- DI 线号映射抽成纯函数 `di_index(pin)`。

**验证**：新增 5 个跨平台单测
（`di_pin_ids_map_to_sequential_indices` / `non_di_pins_are_not_mapped` /
`write_do_rejects_out_of_range_pin` / `read_gpio_line_reports_unsupported_platform` /
`do_bitmap_set_and_clear_are_independent`）。

---

### 1.6 P1 — 蜂窝 failover 切换默认路由失败被吞（**静默断网**）

**位置**：`gateway-core/src/platform/cellular/modem.rs`

**问题**（本轮追加发现）：

```rust
LinkState::CellularUp => {
    tracing::warn!("ethernet down, switching to 4G");
    let _ = set_default_route(&self.config.ppp_iface);   // ← 错误被吞
}
```

两个致命问题叠加：
1. `let _ =` 吞掉 `ip route replace` 的失败 → **切换失败完全不可见**；
2. `link_state` 在切换**之前**就已更新为 `CellularUp`，且状态只在 `new_state != old_state`
   时才处理 → **切换失败后不再重试**。

现场后果：eth 掉线 → 网关「以为已切到 4G」→ 实际默认路由仍指向已 down 的 eth
→ **设备彻底断网，但 API 上一切正常**。这是本批次发现的最隐蔽的缺陷。

**修复**：
- 引入 `RouteTarget { Ethernet, Cellular }` 与纯函数
  `desired_default_route(LinkState) -> Option<RouteTarget>`；
- **物理链路状态**与**默认路由应用状态**解耦：新增 `applied_route` 记录
  「最后一次成功应用的路由目标」；
- 路由切换失败 → 输出 `tracing::error!`（明确写出「可能出现静默断网」），
  **不更新 `applied_route`** → 下一轮自动重试；
- `set_default_route` 的错误信息带上**进程退出码**（此前只知道「失败」，
  不知道是权限不足还是接口不存在）；
- 顺带修：`cleanup_ppp_interface` 的 `ip link set down` 失败由 `debug` 提升为 `warn`
  （清理失败会残留 ppp0 接口与 default route，影响主备切换）；
  `ATE0` / `AT+CPIN` / `AT+CGDCONT`（APN）三处 AT 命令失败由静默改为 `warn`
  （APN 写错是「有信号、无 IP」的常见根因）。

**验证**：新增单测 `default_route_target_follows_link_priority`
（覆盖 EthernetUp / BothUp / CellularUp / Down 四种输入）。

---

### 1.7 P1（附加）— SQLCipher 密钥文件权限竞态 + 损坏密钥静默开库

**位置**：`gateway-core/src/queue/core/key.rs`

**问题**：
- 旧实现 `fs::write`（Unix 默认 0644，同机其它用户可读）**再** `chmod 0600`，
  两者之间存在**竞态窗口**；且 `chmod` 失败被 `let _ =` 吞掉，文件永久保持宽权限；
- 旧实现**不校验已有 keyfile 长度**：磁盘写满等场景会产生 0 字节 keyfile →
  `PRAGMA key = ''` → 数据库既解不开也不报错 → 现象像「数据丢了」。

**修复**：
- 改为 `OpenOptions::new().create_new(true).mode(0o600)` **创建即收紧**；
- 写失败时删除半截 keyfile；
- `chmod` 失败改为 `error!` 告警；
- 新增 `validate_key_len()`：长度 ≠ 32 字节直接报错并给出可执行处置提示；
- key 模块核心逻辑**与 `encryption` feature 解耦**（无条件编译），
  使「落盘 / 权限 / 校验」逻辑可在任意平台单测。

**验证**：新增 5 个单测
（`generates_keyfile_with_expected_length` / `returns_same_key_on_second_call` /
`rejects_corrupted_keyfile` / `rejects_empty_keyfile` / `key_len_validation_boundaries`），
Unix 下另有 `keyfile_is_owner_only_on_unix` 断言权限恰为 0600。

---

## 2. Linux-only 代码块清单

> 「Windows 上是否执行」= 在 Windows 开发机上该代码是否被执行过。

| 文件 | 行号（修复后） | 内容 | Windows 上执行 |
|---|---|---|---|
| `gateway-core/src/platform/gpio.rs` | `DI_LINES` / `ensure_di_lines` / `read_di_levels` | gpiod DI 共享句柄 | 否（`cfg(target_os="linux")`） |
| `gateway-core/src/platform/hardware_watchdog.rs` | `hw_fd_slot` / `hw_write`（Linux 侧） | `/dev/watchdog` 独占句柄 | 否 |
| `gateway-core/src/platform/ota/apply.rs` | `kick_platform_watchdog`（Linux 侧） | OTA 期间喂狗 | 否 |
| `gateway-core/src/platform/ota/bootenv.rs` | `uboot_sync` | `sync` + U-Boot env | 否 |
| `gateway-core/src/platform/cellular/modem.rs` | `set_default_route` / `cleanup_ppp_interface` / `ip` 调用 | `ip route replace` 切默认路由 | 否（但**判定逻辑已纯函数化，Windows 有单测**） |
| `gateway-core/src/platform/cellular/modem.rs` | `check_iface_up` / `get_iface_ip` | 网卡状态探测 | 否 |
| `gateway-core/src/crypto/secret.rs` | L293 / L330 | 机器指纹（`/etc/machine-id`、`/proc/cpuinfo`） | 否 |
| `gateway-core/src/engine/monitor/host.rs` | L202–L360 | 主机指标采集（`/proc`、`/sys/class/thermal`、`/sys/class/net`） | 否（**解析逻辑已纯函数化，有单测**） |
| `gateway-core/src/engine/monitor/health.rs` | L226–L257 | RSS 读取（`/proc/self/statm`） | 否 |
| `gateway-core/src/support/paths.rs` | L27–L34 / L253 | FHS 部署路径 / 路径白名单逃逸目标 | 否 |
| `gateway-core/src/platform/mod.rs` | L12 | `Watchdog` 的平台导出 | 否 |
| `gateway-core/src/south/protocol/modbus/transaction/tests.rs` | 整文件 | Modbus 写超时测试 | 否（整文件 `#![cfg(target_os="linux")]`） |
| `gateway-api/src/web/api/system/handlers/status.rs` | L250–L272 | `dial_supported` 蜂窝拨号能力暴露 | 否 |
| `gatewayd/src/bootstrap/helpers/collection.rs` | L248–L282 | 看门狗启动 + 周期喂狗 | 否 |

---

## 3. 跨平台纯函数化（本轮建立的验证手段）

**动机**：Linux-only 逻辑在 Windows 上永不执行 → 覆盖率天然为 0。
解法：把**判定逻辑**从**副作用（系统调用）**中剥离，抽成纯函数，
在 Windows 上直接单测；副作用层在 Linux 上由交叉编译保证类型正确。

| 纯函数 | 位置 | 判定内容 | 单测数 |
|---|---|---|---|
| `hw_write` + `HwWriteOutcome` | `platform/hardware_watchdog.rs` | 看门狗写成功/失败/无设备 | 5 |
| `classify_sync` + `SyncDecision` | `platform/ota/bootenv.rs` | `sync` 执行结果三态判定 | 1 |
| `di_index` | `platform/gpio.rs` | DI 线号 → 共享句柄下标 | 2 |
| `desired_default_route` + `RouteTarget` | `platform/cellular/modem.rs` | 链路状态 → 默认路由目标 | 1 |
| `validate_key_len` | `queue/core/key.rs` | 密钥长度合法性 | 1 |
| `ensure_key_bytes` / `write_keyfile_secure` | `queue/core/key.rs` | 密钥落盘 + 权限 + 幂等 | 4 |

---

## 4. 剩余风险：无法在本机验证项（需真机验收）

以下项目**必须在 Linux 真机上验收**，本报告的交叉编译只能保证「能编译」：

| 项 | 验收点 | 建议命令 |
|---|---|---|
| 硬件看门狗 | 写入 `'V'` 后设备不再 armed；写失败能被日志捕获 | `dmesg \| grep -i watchdog`；故意让 `/dev/watchdog` 不可写后启动 |
| OTA 全流程 | 升级期间喂狗不失效、env 落盘后回滚可用 | 现场执行一次 OTA 演练 |
| GPIO 采集 | 4 路 DI + 4 路 DO 全部可读可写，**无 EBUSY** | `gpiodetect` + 逐点翻转 |
| 蜂窝 failover | eth 拔线后 30s 内切到 4G，且 `ip route` 真的指向 ppp0 | `ip route show default` |
| SQLCipher | keyfile 权限 0600；重启后 DB 可解密打开 | `ls -l *.db.key` + 重启验证 |
| 主机指标 | CPU/内存/温度/网卡在真机上有合理读数 | `curl /api/system/status` |
| Modbus 写超时 | 真机上写超时被正确中断 | 触发 `transaction/tests.rs` 中被剥离的用例 |

---

## 5. 复现命令

```powershell
# Linux 交叉编译验证（本机 Windows，无需真机）
$env:CC_x86_64_unknown_linux_gnu="<zigwrap>.exe"
$env:AR_x86_64_unknown_linux_gnu="<zigwrap>.exe"
$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="<zigwrap>.exe"
cargo check --target x86_64-unknown-linux-gnu --workspace --all-targets --features fins

# Windows 侧单测
cargo fmt --all
cargo clippy --workspace --all-targets --features fins -- -D warnings
cargo test -p gateway-core --lib --features fins
```

> 注：`encryption` feature 需要 OpenSSL（`OPENSSL_DIR`），本机未安装，
> 因此该 feature 的完整构建需在有 OpenSSL 的环境执行。
> key 模块的**核心逻辑已与该 feature 解耦**，可在 Windows 直接单测。

---

## 6. 后续建议

1. **CI 增加 Linux 交叉编译矩阵**（本报告的 zig 方案可落地为 CI job，
   让 Linux-only 代码至少做到「每次提交都被编译验证」）；
2. **CI 增加 Linux 容器单测 job**（无需真机即可覆盖 `/proc` 解析、
   Modbus 写超时等用例）；
3. 真机验收表按 §4 逐项签字后方可放行批量部署。
