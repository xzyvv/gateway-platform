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
| 修复的 P0/P1 缺陷 | 第一轮 6 项（§1）+ 第二轮 13 项（§8）= **19 项** |
| 部署侧沙箱缺陷（第四轮补记） | **3 项 P0**：配置写回被 systemd 沙箱拦住、GPIO 未放行、串口 DeviceAllow 写成伪路径（见 §10） |
| 新增跨平台单测 | 27 个（§3 + §8.4） |
| Linux 交叉编译验证 | `cargo check --target x86_64-unknown-linux-gnu --all-targets`（三个部署 crate）通过 |
| Windows 侧单测 | `cargo test -p gateway-core --lib --features fins` → **1415 passed / 0 failed** |
| 剩余风险 | §4「无法在本机验证项」+ §10.6，需真机验收 |

**核心结论**：两轮修复的 19 项缺陷全部属于**「错误被静默吞掉」**这一类
（第二轮另有 1 项并发写导致的分区内容交错）。
它们在 Windows 上不会暴露，但会**直接导致设备变砖或现场静默断网**——
即「设备看起来正常，实际已经失去心跳/链路/加密保护」。

> **第四轮补记（2026-11-06）**：前三轮审的是「代码里的 Linux 分支」，第四轮审的是
> 「进程启动后**有没有权限**走到这些分支」——`gateway.service` 的 `ProtectSystem=strict`
> 与 `DeviceAllow` 白名单会把三类功能在 `open()` / `create()` 之前就拒掉，代码全对也没用。
> 结论见 §10。

---

## 1. 已修复缺陷

### 1.1 P0 — 硬件看门狗写失败被静默吞掉（可能变砖）

**位置**：`gateway-core/src/platform/hardware_watchdog.rs`

**问题**：`kick()` / `stop()` / `start()` 三处对 `/dev/watchdog` 的写失败都不做任何处理。
硬件看门狗一旦写失败（设备断电复位后 fd 失效、内核驱动异常），
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

### 1.5 P1 — GPIO 重复 request 同一物理线（EBUSY，DI 点位始终读不到值）

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
| 硬件看门狗（设备名） | 实际设备节点是否为 `/dev/watchdog` / `/dev/watchdog0` / `/dev/softdog`，启动日志是否打印出选中的路径 | `ls -l /dev/watchdog*` + 启动日志中的 `hardware watchdog device detected` |
| OTA 全流程 | 升级期间喂狗不失效、env 落盘后回滚可用 | 现场执行一次 OTA 演练 |
| OTA 回滚判定 | `fw_printenv bootcount` 可读；未定义与读取失败在日志中可区分 | `fw_printenv bootcount`；人为破坏 env 分区看日志是否为 `A/B rollback detection is DISABLED` |
| OTA 分区写入 | 写入超时后**不**出现二次写入；`verify_partition` CRC 能拦住损坏分区 | 在块设备上人为制造写超时（如限速/占用设备），观察日志 `NOT retrying` |
| 优雅停机 | 停机后 `/dev/watchdog` **不再 armed**（不出现停机后的意外复位） | `systemctl restart` 后 `dmesg \| grep -i watchdog` 无超时复位记录 |
| GPIO 采集 | 4 路 DI + 4 路 DO 全部可读可写，**无 EBUSY** | `gpiodetect` + 逐点翻转 |
| 蜂窝 failover | eth 拔线后 30s 内切到 4G，且 `ip route` 真的指向 ppp0 | `ip route show default` |
| SQLCipher | keyfile 权限 0600；重启后 DB 可解密打开 | `ls -l *.db.key` + 重启验证 |
| 主机指标 | CPU/内存/温度/网卡在真机上有合理读数 | `curl /api/system/status` |
| Modbus 写超时 | 真机上写超时被正确中断 | 触发 `transaction/tests.rs` 中被剥离的用例 |
| 心跳僵死检测 | NTP 校时前后心跳均不误报；长时间运行后心跳仍能被判 stale | 校时后观察 `stale_loops` 与 `watchdog_timeout_total` |

---

## 5. 复现命令

```powershell
# Linux 交叉编译验证（本机 Windows，无需真机；zig 作交叉 C 编译器/链接器）
powershell -File scripts/linux_cross_check.ps1

# 或手工
$env:CC_x86_64-unknown-linux-gnu="<zigwrap>.exe"
$env:AR_x86_64-unknown-linux-gnu="<zigwrap>.exe"
$env:CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="<zigwrap>.exe"
cargo check --target x86_64-unknown-linux-gnu --all-targets --features fins \
  -p gateway-core -p gateway-api -p gatewayd

# Windows 侧单测
cargo fmt --all
cargo clippy --workspace --all-targets --features fins -- -D warnings
cargo test -p gateway-core --lib --features fins
```

> 注：`encryption` feature 需要 OpenSSL（`OPENSSL_DIR`），本机未安装，
> 因此该 feature 的完整构建需在有 OpenSSL 的环境执行。
> key 模块的**核心逻辑已与该 feature 解耦**，可在 Windows 直接单测。
>
> `gateway-desktop` 已冻结，其 Tauri 依赖的 `libdbus-sys` 需要 Linux sysroot，
> Windows 侧无交叉 sysroot，不纳入交叉验证范围。

---

## 6. 门禁化：让 Linux 分支不再「静默腐烂」

本次修复的都是**已经被改坏的**代码。若没有持续机制，下次照样会烂。

| 载体 | 变更 |
|---|---|
| `scripts/linux_cross_check.ps1`（新增） | zig 作交叉 C 编译器/链接器 → `cargo check --target x86_64-unknown-linux-gnu --all-targets`（三个部署 crate）。缺 zig / 未装 rust linux target 时**跳过而不阻塞**；`-Probe` 供门禁判定 |
| `scripts/linux/zigwrap.rs`（新增） | 包装器：自动判定 cc / ar 模式；把 rust 三元组换成 zig 三元组；编译成 exe 以规避 cmd.exe 8191 字符命令行上限 |
| `scripts/release_gate.ps1` | 新增 `linux-cross-check` 步骤（门禁共 22 项），前置条件探测失败时标 SKIP 并打印原因 |
| `.github/workflows/ci.yml` | `linux-runtime` job 增加 `--all-targets` 检查步骤（Linux 专属代码改坏了在 CI 就暴露，不必等真机） |

---

## 7. 后续建议

1. **CI 增加 Linux 容器单测 job**（无需真机即可覆盖 `/proc` 解析、
   Modbus 写超时等用例）；
2. 真机验收表按 §4 逐项签字后方可放行批量部署。

---

## 8. 第二轮复审（commit `2c5c4f8`）

> 第一轮修的是「已被改坏的代码」；第二轮回到同一批 Linux 专属代码块重新逐行审，
> 并把审查面从 `cfg(target_os="linux")` **扩展到跨平台的 Linux 相关路径**
> （如队列存储、心跳计时、许可证指纹）。
> 结果：查出 **3 项 P0 + 12 项 P1**，P0 全部修复，P1 修复 10 项
> （余下 2 项为需真机数据的观测项，见 §8.5）。

### 8.1 P0 — 优雅停机时硬件看门狗未解除（停机后被误复位）

**位置**：`gatewayd/src/bootstrap/helpers/shutdown.rs`

**问题**：解除 armed 的 `stop_platform_watchdog()` 放在
`graceful_shutdown_inner` 的**最后一行**，而该函数被外层
`tokio::time::timeout(80s)` 包裹。一旦总宽限命中（某个子阶段卡死），
inner future 被 drop → **最后一行永不执行** → `/dev/watchdog` 保持 armed。
现场表现：优雅停机后被 systemd 拉起，硬件看门狗仍在计时且无人喂狗 →
整机被误复位，表现为「重启后莫名其妙又重启一次」，日志里没有任何线索。
inner panic（栈展开）同样走不到最后一行。

**修复**：新增 `WatchdogDisarmGuard`，`Drop` 时调用 `stop_platform_watchdog()`；
守卫放在**外层** `graceful_shutdown`，因此三条路径全覆盖：
inner 正常返回 / 外层 timeout 命中 / inner panic 栈展开。
`stop_platform_watchdog()` 本身幂等（fd 被 take 后二次调用是 no-op）。

### 8.2 P0 — OTA bootloader env 读取失败被吞（A/B 回滚失效 → 变砖）

**位置**：`gateway-core/src/platform/ota/bootenv.rs`

**问题**：`uboot_printenv()` 原实现用 `.ok()?` 吞掉 spawn 失败与读取失败，
返回 `None`。而 `None` 与「变量未定义」在上层**不可区分**：
`bootcount` 静默变成 `None` → `check_bootcount_rollback()` 永远返回 false →
**需要回滚的新分区被反复加载** → 设备变砖且无人察觉。
（对照：`fw_printenv` 在「变量不存在」时也返回非零退出码并打印
`## Error: "bootcount" not defined`，这是**正常情形**，不能当成故障。）

**修复**：抽出纯函数
`classify_printenv(spawn_ok, success, stdout, stderr, name) -> PrintenvOutcome`，
三态区分 `Value / Undefined / Unavailable`；`Unavailable` 分支打 `error!`
并明确写出「A/B rollback detection is DISABLED」。

### 8.3 P0 — `reboot` 的 spawn 失败与退出码被吞（以为重启了，其实没重启）

**位置**：`gateway-core/src/platform/ota/bootenv.rs`

**问题**：旧实现 `let _ = Command::new("reboot").status();` **同时吞掉
spawn 失败与退出码**。在 systemd 管理的容器 / minimal rootfs 上
`reboot` 常常不在 PATH（或需要 root），此时 `perform_rollback` 以为
重启成功返回 `Ok`，但设备**根本没有重启** → 继续运行故障分区 →
现场表现为「设备重启后还是坏的」，而日志里没有任何线索。

**修复**：抽出纯函数 `classify_reboot(spawn_ok, success) -> RebootOutcome`
（`Initiated / SpawnFailed / Failed`）；失败一律上抛 `GatewayError::Io`。

### 8.4 P0 — OTA 分区写入超时后重试（并发写同一设备 → 分区内容交错 → 变砖）

**位置**：`gateway-core/src/platform/ota/apply.rs`

**问题**：`write_partition` 超时后进入下一次 attempt，而 attempt 会
**第二次 open 同一块设备**。但 `spawn_blocking` 派生的阻塞任务
**不可取消**：`tokio::time::timeout` 只丢弃等待句柄，底层写仍在继续
（持有 fd、正在 `write_all`）。于是两个写者并发写同一设备 →
字节交错 → 分区内容是「两次固件的混合体」。
`verify_partition` 只读回 `payload_len` 字节，CRC 校验可能**恰好通过**
（头部字节来自同一个 payload），但后半段是垃圾 → 设备变砖。

**修复**：超时即视为**写入结果不确定**，直接上抛 `TimedOut`，**不再重试**；
由上层 `verify_partition` 的 CRC 读回校验作为最后一道防线。

### 8.5 P1 — 其余 10 项修复

| # | 位置 | 问题 | 修复 |
|---|---|---|---|
| 1 | `ota/apply.rs` | 新固件比旧固件短时，旧固件尾部残留被当作新固件的一部分（「新头 + 旧尾」混合体） | 普通文件写完 `set_len` 截断到 payload 长度（块设备不支持 `ftruncate`，由 `verify_partition` + 真机验收覆盖） |
| 2 | `ota/apply.rs` | `if let Ok(bytes) = fs::read(&marker)` 把「读失败」与「文件不存在」等同处理，且**无任何日志** → pending_reboot 复核逻辑被静默跳过 | 区分 `NotFound`（正常）与其它错误（`error!` 告警），拆出 `handle_pending_reboot_marker` |
| 3 | `ota/apply.rs` | `perform_rollback` 里有**连续三次** `sync()`（第三次与第二次完全重复），多起一个子进程 | 去掉重复的第三次 `sync()` |
| 4 | `platform/hardware_watchdog.rs` | 二次 `Watchdog::start` 会把**已有**的硬件 fd 覆盖掉：Linux 下 `/dev/watchdog` 独占，二次 open 必 `EBUSY` → `hw_fd` 为 `None` → 旧 File 被 drop → **fd 被关闭**，硬件看门狗失去喂狗通道（设备仍 armed，但没人写心跳）→ 整机超时被复位 | 槽位已有句柄时**保留旧的，绝不覆盖**，并 `error!` 告警 |
| 5 | `platform/hardware_watchdog.rs` | 软件看门狗线程 spawn 失败被 `handle.ok()` 静默吞掉 → 软件看门狗**根本不存在**，进程卡死不会被 abort，且日志无任何线索 | spawn 失败改为 `error!` 告警，明确写出「the software watchdog is NOT running」 |
| 6 | `gatewayd/.../collection.rs` | 硬件看门狗设备路径**硬编码** `/dev/watchdog`；多路看门狗板卡上内核创建的是 `/dev/watchdog0`、`/dev/watchdog1`，少数板卡用 `/dev/softdog` → open 失败 → 只剩软件看门狗 → 整机卡死不会被复位 | 候选列表按优先级探测（`/dev/watchdog`、`/dev/watchdog0`、`/dev/watchdog1`、`/dev/softdog`），判定逻辑抽成纯函数 `select_watchdog_device` |
| 7 | `gatewayd/.../collection.rs` | 硬件喂狗循环用裸 `tokio::spawn` —— task panic 被 tokio **静默吞掉**（JoinHandle 被丢弃）→ 喂狗停止且日志无线索 → 15s 后整机被硬件看门狗复位 | 改用 `spawn_catch_unwind("hw_watchdog_kick", ...)`，panic 记 error 日志并计入 `task_panic_total` |
| 8 | `queue/core/task.rs` | `dequeue_batch(limit).unwrap_or_default()` 把 DB 错误伪装成「队列为空」→ 上层拿到空批，无法区分「确实没有数据」与「DB 查询失败」→ **北向静默无数据**（采集正常、MQTT 收不到、日志无异常） | 区分 `Ok(Ok)` / `Ok(Err)`，`Ok(Err)` 分支 `error!` 告警 |
| 9 | `queue/core/task.rs` | enqueue 把 `Vec<DataPoint>` **move** 进闭包；闭包 panic / task 被取消时 `spawn_blocking` 返回 `Err(JoinError)`，points 已被消费 → 该分支**无法回退到 fallback 缓冲** → 整批数据静默丢失 | points 改用 `Arc<Vec<DataPoint>>` 共享，JoinError 分支也能把整批点回退进降级缓冲（与 `Ok(Err)` 分支行为对齐） |
| 10 | `queue/storage/sqlite/*.rs` + `engine/error.rs` | ①`release_in_flight` 吞掉错误只打 warn，调用方仍无条件 `saturating_dec(ids.len())` → 内存计数**低估** → `MAX_IN_FLIGHT` 反压阈值被提前放宽，且 DB 里那些行仍是 `in_flight = 1` → 在 dequeue 的 `WHERE in_flight = 0` 里**永不入选** → 数据滞留不投递（现场「采集正常但北向数据越来越旧」）。②磁盘满判定只做**字符串匹配**，rusqlite/SQLite 升级或措辞变化即失效 → 失去 30s 退避保护 | ①`release_in_flight` 返回**实际复位行数**并上抛错误，调用方按实际行数扣减（失败保留计数）。②`From<rusqlite::Error>` 把**主错误码**编进消息前缀 `sqlite_error(code=N)`，判定优先按码（`SQLITE_FULL=13` / `SQLITE_IOERR=10`），文案匹配仅作兜底 |

### 8.6 第二轮建立的跨平台纯函数与单测

| 纯函数 | 位置 | 判定内容 | 单测数 |
|---|---|---|---|
| `classify_printenv` + `PrintenvOutcome` | `platform/ota/bootenv.rs` | `fw_printenv` 三态：值 / 未定义 / 不可用 | 1 |
| `classify_reboot` + `RebootOutcome` | `platform/ota/bootenv.rs` | `reboot` 三态：已发起 / spawn 失败 / 退出码非零 | 1 |
| `select_watchdog_device` | `gatewayd/.../collection.rs` | 看门狗设备候选路径选取 | 1 |
| `is_usb_backed_path` | `license/fingerprint.rs` | sysfs 路径是否由 USB 承载（`usb<N>` 控制器目录） | 1 |
| `sqlite_code_from_message` | `queue/core/task.rs` | 从 `sqlite_error(code=N)` 前缀解析主码 | 1 |
| `sqlite_error_code` | `engine/error.rs` | `rusqlite::Error` → SQLite 主码（`extended_code & 0xff`） | 2 |
| `now_ms` | `engine/monitor/heartbeat.rs` | 心跳计时用单调钟且恒 ≥ 1 | 1 |

> **心跳计时（`now_ms`）的修复**：旧实现用挂钟 `SystemTime`。挂钟会被 NTP 校时影响，
> 两种故障都极难排查：
> - **前跳**（12:00 → 13:00）：`elapsed` 瞬间变成 3.6e6 ms → 所有心跳**同时**
>   被判 stale → 误报僵死 + 误计 `watchdog_timeout_total`，还可能触发自愈
>   执行器去重启**健康**的服务；
> - **回拨**（手动校回过去）：`saturating_sub` 得 0 → elapsed 恒为 0 →
>   **僵死检测彻底失效**（最危险：进程真卡死了也不会有人发现）。
>
> 改用 `Instant`（单调钟），与 `platform::hardware_watchdog::mono_millis` 口径统一。
> 返回值 +1，避免与「从未 kick」的 `last_kick == 0` 哨兵冲突。

> **许可证指纹（`is_usb_backed_path`）的修复**：USB 网卡（USB-Ethernet dongle）
> 在 sysfs 下同样有 `/sys/class/net/<n>/device` 节点，且接口名通常是 `eth1` /
> `enx0011...` 这类**不以 `usb` 开头**的名字 → 接口名过滤拦不住它。
> 现场后果：运维插拔一个 USB 网卡 → 板载 MAC 指纹变化 → **许可证指纹漂移** →
> 已授权设备被判未授权（现场「授权莫名失效」）。
> 判定方式：USB 设备的 sysfs 路径必然经过某个 USB 控制器目录（`usb1` / `usb2`），
> 因此按 `usb` + 纯数字序号的路径段匹配（不能简单匹配「含 usb 的段」，
> `usb_controller` 之类的普通目录名会误判）。

### 8.7 第二轮未修复项（需真机数据）

| 项 | 原因 |
|---|---|
| OTA 写分区超时后的实际残留状态 | 需要真机块设备（`/dev/mmcblk0p*`）上复现超时才能观察残留字节；本机只能验证「不再重试」这一逻辑 |
| 块设备上的 `set_len` 截断 | 块设备不支持 `ftruncate`（`EINVAL`/`ENOTTY`），已按文件类型判定跳过；截断效果需真机验证 |

### 8.8 第二轮验证结果

| 项 | 结果 |
|---|---|
| `cargo test -p gateway-core --lib --features fins` | **1415 passed / 0 failed / 1 ignored** |
| `cargo test -p gatewayd --features fins` | **8 passed / 0 failed** |
| `cargo clippy -p gateway-core -p gatewayd --all-targets --features fins -- -D warnings` | 干净 |
| `cargo fmt --all -- --check` | 干净 |
| Linux 交叉 `cargo check`（zig 0.14.1，三个部署 crate，`--all-targets`） | 通过 |
| 提交 | `2c5c4f8`（13 files, +805 / −56） |

### 8.9 第二轮修复的长周期回归验证（soak）

本轮修复涉及**采集 → 管道 → 队列 → 北向**主链路（`task.rs` 的 enqueue 改
`Arc<Vec<DataPoint>>` 共享、`dequeue_batch` 错误分支区分、RSS/句柄口径），
因此用长周期 soak 做了回归验证。

| 轮次 | 时长 | 二进制 | 判定 | 关键读数 |
|---|---|---|---|---|
| 基线 | 3h（10800s） | **修复前** | 通过（27/27） | 北向 3702500 点 / 100.0000%；RSS 末段斜率 0.82 MB/h；句柄增量 +0 |
| 回归 | 2h（7200s） | **修复后**（`2c5c4f8` + `1761e97`） | 通过（27/27） | 北向 2417000 点 / 100.0000%（Good 占比 100%）；RSS 增长 1.5 MB（1.07×）；RSS 末段斜率 0.82 MB/h；句柄增量 +0（基线 105）；panic +0；watchdog 超时 +0 |

两轮均注入同样的 5 段故障（单从站静默 / 整总线静默 / 慢响应未超时 /
慢响应超时 / 异常码应答），回归轮的 `collect_fail` 增量与故障窗口精确对应、
故障结束后停止增长，队列全程 0（无积压、无残留）。

> **重要说明**：3h 基线跑在修复**之前**的二进制上，只能说明「修复前的代码本来
> 就是稳的」，**不能**证明修复没有引入回归 —— 这正是追加 2h post-fix soak 的原因。
>
> soak 只替换物理串口（`SoakPort` 桩），其余为生产代码路径；队列用
> `MemoryStorage`，**未覆盖 SQLite 后端**，也**未覆盖** Linux fd 语义、
> NTP 校时与 Linux 内核专属接口（见 §4 / §8.7 真机验收表）。

---

## 9. 第三轮：全量 cfg 扫描 + ARM 产物链路编译验证（r16）

> 前两轮（§1 / §8）是**按已知缺陷线索**复审；本轮改为**机械全量扫描**：
> 把全仓所有 `cfg(target_os="linux")` / `cfg(unix)` / `cfg(not(windows))` /
> `cfg!(target_os="linux")` 逐条过目，再补做此前从未验证通过的 ARM 产物链路。

### 9.1 全量 cfg 扫描结果

| 项 | 结果 |
|---|---|
| 扫描到的 Linux 分支点 | **60 处**（覆盖 `cfg(...)` 属性与 `cfg!(...)` 宏两种形态） |
| §2 清单已覆盖 | 14 处 |
| 本轮补审（§2 未列） | `support/cron.rs` / `crypto/cert.rs` / `config/loader.rs` / `engine/shutdown.rs` 等 4 个文件全读 |
| 新发现的「错误被静默吞掉」缺陷 | **0 项** |
| 新发现的其它缺陷 | 1 项（P2 权限，见 §9.2） |

**结论**：按 r15 判据（缺陷会导致设备变砖 / 静默断网 / 保护机制失效），
Linux 专属代码路径**未发现新的静默失效点**。§2 清单外的 4 个文件逐一读完，
其 Linux 分支要么是纯数据变换（cron 解析、证书读取），要么的错误处理与
跨平台路径一致，不构成「只在 Linux 上才静默」的缺陷。

### 9.2 P2 — 看门狗快照文件权限过宽（已修）

**位置**：`gateway-core/src/platform/watchdog_flush.rs`

**问题**：`write_snapshot()` 用 `std::fs::write` 落盘 `watchdog_state.json`，
Unix 默认权限 **0644**（同机其它用户可读可改）。

**影响**：快照内容不含凭据（仅时间戳 / 队列水位 / OTA 状态），泄露后果有限；
但它记录的是「上次是卡死 abort 还是正常退出」—— **现场排障依据**，
可被任意本地用户篡改会让排障结论失真。定级 P2。

**修复**：`std::fs::write` → `OpenOptions` + `#[cfg(unix)] opts.mode(0o600)`，
**创建即 0600**（不走「先宽后紧再 chmod」，避免 chmod 竞态窗口）。
新增 Unix 单测 `flush_once_snapshot_is_owner_only` 断言权限恰为 `0o600`。

### 9.3 ARM 产物链路：此前从未验证通过，现已四条全部通过

**「未验证」的真实含义**：CI 的 `cross-arm` 是重作业（`workflow_dispatch` 或
`vars.RUN_HEAVY == 'true'` 才跑），且 GitHub Actions 额度已耗尽 →
**ARM 代码路径长期没有被真正编译过**。本地虽有 `scripts/arm_cross_check.ps1`，
但 zig 交叉链路从未成功产出 release 产物。

**本轮修掉的三个根因**（都在 `scripts/linux/zigwrap.rs`）：

| # | 现象 | 根因 | 修法 |
|---|---|---|---|
| 1 | `unable to find dynamic system library 'util' ... util.dll` | rustc **只在编译阶段**传 `--target=`，**链接阶段不传** → zig 按宿主 Windows 找库 | ps1 注入 `ZIG_TARGET`；zigwrap 在链接器模式下补 `-target <zig 三元组>` |
| 2 | `unsupported linker arg: --fix-cortex-a53-843419` | rustc 的 aarch64 gnu/musl target spec **无条件**注入该缓解参数；zig cc 的 `-Wl,` 有白名单，白名单外直接 `fatal` | gnu 分支由 zigwrap **丢弃**该参数并 stderr 留痕；musl 分支改 `-C linker-flavor=ld.lld` 直驱 rust-lld，因 `pre_link_args` 只对 Gnu flavor 生效 → 天然不注入 |
| 3 | `unable to parse target query 'arm-unknown-linux-musleabihf'` | cc-rs 的 build.rs 传**不带版本号**的 rust 三元组，zigwrap 映射表缺 musl 变体 | 补 `armv7-/arm-` × `musleabi/musleabihf` 共 4 条映射 |

> 根因 2 的绕过手段实测（zig 0.14.1）全部无效：`-Wl,--fix-cortex-a53-843419`、
> `-Xlinker --fix-cortex-a53-843419`、裸 `--fix-cortex-a53-843419`、
> `-fuse-ld=lld -Wl,--fix-cortex-...` 全部 fatal。白名单校验发生在
> **参数解析阶段**，早于链接器选择，无法绕过。
>
> **丢弃该参数不影响线上行为**：现场产物由 `deploy/build-release.sh` /
> CI 的 cross-arm 作业用 Ubuntu 原生 `aarch64-linux-gnu-gcc` 生成，
> GNU ld（binutils ≥ 2.33）**支持** `--fix-cortex-a53-843419`，
> 缓解照常生效；本地 zig 链路只是**编译验证门禁**，产物不部署。

**验证结果**（zig 0.14.1 + rustc 1.98.1，`-Release -Strict`）：

| 链路 | 产物 | 校验 | 结果 |
|---|---|---|---|
| `aarch64-unknown-linux-gnu` | 21 MB | ELF magic + `e_machine=0xB7` | ✅ |
| `armv7-unknown-linux-gnueabihf` | 21.4 MB | ELF magic + `e_machine=0x28` | ✅ |
| `aarch64-unknown-linux-musl` | 19.5 MB | `e_machine=0xB7` + 无 `.dynamic` 段 + 无 `GLIBC_2.xx` 引用 | ✅ |
| `armv7-unknown-linux-musleabihf` | 20.8 MB | `e_machine=0x28` + 无 `.dynamic` 段 + 无 `GLIBC_2.xx` 引用 | ✅ |
| `x86_64-unknown-linux-gnu`（`cargo check --all-targets`） | — | 三个部署 crate | ✅ |

> musl 的两项检查（无 `.dynamic` 段、无 `GLIBC_2.xx` 引用）是「现场设备
> glibc 版本千差万别」场景的关键判据 —— gnu 动态产物带着构建机的 glibc
> 版本要求，在 Debian10(2.28) / Debian11(2.31) / Yocto musl 上直接
> `GLIBC_2.xx not found`，这是现场最典型的「装上后无法运行」。

### 9.4 门禁脚本自身修掉的两个缺陷

| 位置 | 缺陷 | 修法 |
|---|---|---|
| `scripts/arm_cross_check.ps1` L185-189 | 注释称「zig cc 白名单不认这个参数…改用 ld.lld flavor 直接驱动 rust-lld 即可绕开」—— 与实际机制不符（gnu 分支实际靠 zigwrap 丢弃参数通过，musl 分支才是 rust-lld 直驱） | 改写注释，分别说明 gnu / musl 两条**不同**的绕过路径 |
| `scripts/arm_cross_check.ps1` L139-147 | `-MuslOnly` 对**默认的 gnu target 列表**做过滤 → 结果必为空 → **musl 链路恒为「跳过」**，从未真正验证过 | 改为「优先用调用方显式传入的 musl target，否则回落到默认 musl 列表」 |

### 9.5 第三轮验证结果

| 项 | 结果 |
|---|---|
| `cargo test -p gateway-core --lib --features fins` | **1442 passed / 0 failed / 1 ignored** |
| Linux x86_64 交叉 `cargo check --all-targets` | 通过 |
| ARM gnu release 产物（2 个 target） | 通过（架构校验） |
| ARM musl release 产物（2 个 target） | 通过（架构 + 静态链接校验） |

---

## 10. 第四轮：部署侧 systemd 沙箱与设备白名单（r17，2026-11-06 补记）

> **本轮视角切换**：前三轮都在审「代码里的 Linux 分支对不对」；本轮审的是
> 「进程启动后，**有没有权限**走到这些分支」。systemd 的 `ProtectSystem=strict`
> 与 `DeviceAllow=` 白名单在 `open()` / `create()` **之前**生效，代码全对也没用，
> 而且现场表现与前三轮同类：**静默失效**（只有一条 warn 或 EPERM，容易被判成硬件故障）。
>
> 三条缺陷都在 `gateway-core/deploy/gateway.service` 与 `install.sh` 的权限配置里，
> 与 Rust 代码逻辑无关，因此在 Windows 开发机与 x86 CI 上运行单测无法发现。

### 10.1 P0 — Web API 在线改配置在真机上必然落盘失败

| 环节 | 事实 |
|---|---|
| 默认落盘路径 | `/etc/gateway/config.json`（`gateway-core/src/support/paths.rs`） |
| 落盘实现 | `atomic_write()`，临时文件建在**目标同目录** `/etc/gateway/`，需要该目录对 `gateway` 用户**可写** |
| 目录权限（修复前） | `chown -R root:gateway` + `chmod -R g+rX,o-rwx` → 组只有 `r-x`，**无 `w`** |
| systemd 沙箱（修复前） | `ProtectSystem=strict` 令 `/etc` 只读，而 `ReadWritePaths` 只含 `/var/lib/gateway /var/log/gateway /tmp`，**不含 `/etc/gateway`** |
| 唯一写回路径 | `reload_then_save()`：先 reload + 更新内存 → 再 `save()` → 失败返回 500 `runtime reloaded but disk save failed` |

**后果**：Web 端改点表 / 总线 / 上行 / 认证 → 界面提示成功、运行时已生效，
**磁盘未更新**，设备一重启配置全部回滚。受影响端点：config / points / groups /
assets / virtual_points / find-replace / uplink / auth。

**修复**：
- `gateway.service` 的 `ReadWritePaths` 增加 `/etc/gateway`；
- `install.sh` 保持 `root:gateway` 属主，目录改为 `g+rwx`（`find … -type d -exec chmod g+rwx,o-rwx`），
  配置文件本身仍 `640` —— `atomic_write()` 走「同目录建 tmp → rename 覆盖」，只需要目录写权限，不需要文件写权限。

### 10.2 P0 — DeviceAllow 缺 `/dev/gpiochip0`，GPIO 全线 EPERM

**证据**：代码硬编码 `gpiod::Chip::new("/dev/gpiochip0")`（`platform/gpio.rs`），
GPIO 协议走独立采集路径、不经 `BusManager`（`gatewayd/src/bootstrap/helpers/collection.rs`）；
而 `DeviceAllow=` 一旦出现任意条目就是**白名单（closed）语义**，未列出的设备一律拒绝。

**后果**：DI 采集、DO 输出、**RS485 DE 方向控制**在 `open()` 之前就被 EPERM 拒绝 →
现场表现为「点位一直不更新」「DO 不动作」，日志里可能只是一条 warn。

**修复**：`DeviceAllow=/dev/gpiochip0 rw`（`gpiod` 对同一批物理线复用 fd，
代码已用全局 `OnceCell` 处理 EBUSY，无需改动）。

### 10.3 P0 — `DeviceAllow=/dev/ttyUSB`、`/dev/ttyS` 是路径形式但不是真实设备节点

**证据**：systemd 规范的 `DeviceAllow=` 只接受**真实设备节点路径**或
`char-<组名>` / `block-<组名>` 设备组形式；通配只对设备组名生效。
systemd v255 的 `bpf_devices_allow_list_device()` 先 `device_path_parse_major_minor()`，
失败再 `stat()`，**路径不存在则 warning 后整条丢弃**，不会展开成前缀匹配。
`/dev/ttyUSB`、`/dev/ttyS` 都不是真实节点（真实节点是 `/dev/ttyUSB0`、`/dev/ttyS1` …）。

**后果**：串口设备在 `open()` 前被 EPERM 拒绝 → 全部串口协议采集不上来。

**修复**：改用设备组形式 `DeviceAllow=char-ttyUSB rw` / `char-ttyS rw` / `char-ttyACM rw`
（一次放行该 major 的全部次设备，不用维护端口号）；组名解析依赖驱动已加载，
故 `install.sh` 会把 `cp210x / ch341 / ftdi_sio / pl2303 / cdc_acm` 写入
`/etc/modules-load.d/gateway-serial.conf` 并立即 `modprobe`。

**连带项**：OTA A/B 分区 `/dev/mmcblk0p2` / `/dev/mmcblk0p3` 硬编码 eMMC，
现场若用 SD / NVMe 作 OTA 介质会直接失败。已改为参数化：
`gateway.service` 的分区段落包在 `# OTA-PARTITIONS-BEGIN/END` 标记里，
`install.sh` 按 `GATEWAY_OTA_SLOT_A` / `GATEWAY_OTA_SLOT_B` 同时改写
`DeviceAllow` 与 `Environment=`（两处缺一，OTA 必然失败）。

### 10.4 门禁（P0-4）：C9~C12 + 真实写回冒烟

这三条缺陷的共同点是「改完就忘、下次又漂」，所以全部转成 `scripts/deploy_lint.ps1` 的静态断言：

| 断言 | 拦截 |
|---|---|
| C9 代码里 open 的设备路径 ⊆ `gateway.service` 的 `DeviceAllow` | 10.2 / 10.3 |
| C10 `ReadWritePaths` ⊇ 配置目录（从 `paths.rs` 的默认路径反查） | 10.1 |
| C11 `install.sh` 的配置目录权限与 service 写权限一致（组可写 ⇔ `ReadWritePaths` 含该目录） | 10.1 |
| C12 `DeviceAllow` 不得出现「路径形式但非真实节点」的裸设备名（`/dev/ttyUSB` 这类） | 10.3 |

并且 `ci.yml` 的 `deploy-lint` job 已改为真跑 `pwsh scripts/deploy_lint.ps1`（此前只跑
actionlint + shellcheck，**根本没调用过这个脚本**）。

静态断言只能证明「文本一致」，证明不了「权限对了但就是写不进去」，因此 `deploy-smoke`
增加一条**真实写回**链路：登录 → `POST /api/config` 把启动期固化字段
`web.realtime_buffer` 从 1024 改成 4096 → 断言磁盘 `config.json` 内容确实变化且新值存在
→ `systemctl restart` → 断言重启后磁盘仍是 4096、回读接口也是 4096。
选「启动期固化字段」是因为它**只改内存不会体现**，能区分「改了内存」与「改了磁盘」。

### 10.5 本轮验证结果

| 项 | 结果 |
|---|---|
| `scripts/deploy_lint.ps1` | **121 项全过 / 0 失败 / 0 警告**（C9~C12 + 后续 C14~C20） |
| `deploy-smoke`（真实 systemd + 真实安装） | GitHub Actions 实跑全部通过（run `42375783484`）：安装 → 启动 → 配置写回 → 重启存活 → 健康度 31 通过 0 FAIL |
| C9~C12 反向验证 | 4 个变异逐条还原缺陷，对应断言均报错：删 `ReadWritePaths /etc/gateway` → C10（+C11 交叉告警）；删 `DeviceAllow=/dev/gpiochip0` → C9；`char-ttyUSB` 写回 `/dev/ttyUSB` → C12（+C9 设备覆盖告警）；`install.sh` 目录改回 `g+rX` → C11 |

### 10.6 本轮剩余风险

1. **`DeviceAllow=char-*` 依赖驱动已加载**：若现场内核把驱动编译进内核（非模块），
   `/etc/modules-load.d` 不生效但 `/proc/devices` 仍有组名，不影响；
   若驱动是模块且 `modprobe` 失败，systemd 解析组名失败 → 整条丢弃 → 串口 EPERM。
   现场验收需确认 `lsmod | grep -E 'cp210x|ch341|ftdi_sio'` 与 `/proc/devices` 里的 `ttyUSB`。
2. **`ReadWritePaths=/etc/gateway` 会放宽 systemd 对 `/etc` 的只读保护**：范围仅限该目录，
   且目录内文件仍 `640 root:gateway`，风险可控；但不能在此基础上再扩大。
3. **OTA 分区参数化只覆盖 `DeviceAllow` 与 `Environment=` 两处**：若现场用 SD/NVMe，
   还要确认 U-Boot 侧的 A/B 槽位定义同步（属真机验收项，见 §4）。

### 10.7 P1 补强：交付验收脚本的硬件探测口径落后于代码

§10.2~10.4 把「代码与 unit 的一致性」钉死在静态断言里，但**验收脚本自己**的口径没人管：

| 缺陷 | 现象 |
|---|---|
| `verify-deploy.sh` 只查 `/dev/watchdog` | 代码早已支持 `watchdog0` / `watchdog1` / `softdog` 四个候选（§9.2 第 6 条），但验收脚本在 watchdog0 板卡上判「无硬件看门狗」→ **验收结论是错的** |
| 脚本全无 `DeviceAllow` 核对 | §10.2/10.3 那类「设备在、unit 没放行 → open() 之前 EPERM」的问题，验收脚本一个都查不出；而这类故障的表现是**服务正常、控制台能开、硬件功能却是空的**，现场几乎无法定位 |
| 脚本全无 OTA / journald 检查 | OTA 分区路径与实际介质不匹配（§10.6 第 3 条的遗留风险）要到升级当场才暴露；journald 无 `SystemMaxUse` 会让日志吃满数据盘 → SQLite 入库暂停 → **表现为数据停采而非报错** |

修复（只改验收脚本与门禁，不动运行时行为）：

1. **看门狗口径对齐**：脚本按代码同款候选顺序探测，并报出「代码实际会选中哪一个」。
2. **新增 [6c] DeviceAllow 双向核对**：
   - 正向：unit 放行的路径在本机是否存在（不存在 → 该条是摆设）
   - 反向：本机存在的关键硬件（看门狗 / GPIO / 串口 / OTA 分区）是否真的被放行
   - 按 systemd 语义还原匹配规则：路径形式精确匹配，`char-<组名>` 形式匹配该组次设备
   - **OTA 分区取 `Environment=GATEWAY_OTA_SLOT_A/B` 里的路径**（代码读的就是这个环境变量），
     DeviceAllow 只是放行侧；两者都指向真实节点才算通过
3. **新增 [6d] journald 限额检查** + 数据分区剩余空间换算（对应 §9.1 队列触顶与磁盘不足两条停采路径）
4. **新增门禁 C25**：把「脚本里的设备路径」与「代码里的常量」双向钉死 ——
   看门狗候选集合**及其顺序**、`gpiod::Chip::new(...)` 的 GPIO 路径、
   OTA 默认分区（代码 default ⇄ `install.sh` 写进 unit 的默认值）。
   任一侧增删而另一侧没同步 → C25 FAIL。

> 顺序也要对齐：代码按候选顺序取**第一个存在的**路径，脚本若选错优先级会报出另一个设备，
> 结论同样不可信。

> OTA 介质匹配的判据也曾经历过一次修订：**不能用「剥掉尾部数字」当同族判据**——
> 那会把整盘 `/dev/mmcblk0` 削成 `/dev/mmcblk`、`/dev/nvme0n1` 削成 `/dev/nvme0n`，
> 媒体匹配就失真了。脚本改为按命名族分别推导所属整盘
> （`pN` 族 `mmcblk0p2 → mmcblk0` / `nvme0n1p2 → nvme0n1`；`sdX N` 族 `sda1 → sda`；
> 整盘名 `mmcblk0` / `nvme0n1` 原样保留），且判定用**同族磁盘**而非「有无任意块设备」——
> 否则没装 eMMC、只有 `/dev/sda` 的 CI runner 会被误判成部署失败。

### 10.8 P1 补强：journal 磁盘限额没落地（且不能写在 unit 里）

§10.7 第 3 条只补了**验收脚本**（`[6d]` 段查 `SystemMaxUse`），但**限额本身从未被落地**——
脚本只会报一句「未设置」，装完还是没设，磁盘照样会被日志吃满。而这条路径的后果是
**数据停采而不是报错**（§9.1 队列触顶 + 磁盘不足），现场几乎无法定位。

这里有一个反直觉的要点，也是最容易改错的地方：

| 缺陷 | 现象 |
|---|---|
| `SystemMaxUse=` 写进 `gateway.service` | **静默不生效**。它是 `journald.conf` 的 `[Journal]` 段选项，unit 里没有 `[Journal]` 段，systemd 直接忽略。看着「配了限额」，实际 journal 无上限——比不配更危险 |
| 只查 `/etc/systemd/journald.conf` 主文件 | 若按官方推荐用 drop-in 落地，验收脚本反而报「未设置」→ 现场误以为没生效，反复排查 |
| 只有限额、没有 unit 级限流 | 限额只兜住「总量」，兜不住「速率」。异常日志激增时 journal 会在几分钟内冲到上限，数据盘瞬间被占满 |
| 只有 unit 级限流、没有限额 | `LogRateLimit*` 丢弃超限消息，但**不限制磁盘占用**——总量仍会持续增长直至磁盘占满 |

修复（三段缺一不可）：

1. **`gateway.service`**：加 unit 级 `LogRateLimitIntervalSec=30s` + `LogRateLimitBurst=1000`。
   它覆盖 journald.conf 的全局 `RateLimit*`（全局默认 30s/10000 条，且实际阈值还要按可用磁盘
   空间再乘一个倍率，磁盘越空倍率越小、反而越容易触发丢弃）。1000 条/30s 远高于正常量级
   （正常运行是生命周期/事件级，每分钟个位数到几十条），不会误杀正常日志。
2. **`install.sh` 新增 [6.5/7] 步**：写 drop-in
   `/etc/systemd/journald.conf.d/60-gateway-journal.conf`（`SystemMaxUse` / `SystemKeepFree` /
   `RuntimeMaxUse`），并重启 `systemd-journald` 生效。
   用 drop-in 而不是改主文件：不污染发行版自带的 `journald.conf`，且 drop-in 优先级高于主文件；
   `60-` 前缀压得过发行版常见的 `10-40` 号 drop-in（同名选项按文件名字典序取最后一个）。
3. **`verify-deploy.sh` 的 `[6d]` 段**：改为解析**生效值**——主文件（`/etc` > `/run` >
   `/usr/local/lib` > `/usr/lib`，第一个存在的生效）+ 全部 drop-in（按 basename 排序、
   同 basename 取高优先级目录，覆盖主文件），同时核对 `SystemKeepFree` 与 unit 级
   `LogRateLimit*`。

> 顺带把 `C7` 的服务名一致性检查开了一个**系统 unit 白名单**：install.sh 现在会
> `systemctl restart systemd-journald`，它本来就与 `SyslogIdentifier` 无关。
> 白名单显式列出，避免以后引入系统 unit 时被当成「查错了服务」。

新增门禁 **C26**，把这条链路钉死：unit 有 `LogRateLimit*`、unit 里**没有**误写
`SystemMaxUse=`、install.sh 会写 drop-in 且含 `SystemMaxUse`/`SystemKeepFree`、
verify-deploy.sh 会查 drop-in 且会核对 unit 限流。任一段被删 → C26 FAIL。


