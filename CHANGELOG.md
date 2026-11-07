# Changelog

本文件记录所有**对交付有影响**的变更。格式参考
[Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循
[Semantic Versioning](https://semver.org/lang/zh-CN/)。

维护约定：

- 版本号与 `Cargo.toml` 的 `[workspace.package].version` 保持一致；打 `v*` tag 会触发
  `release-gate.yml`（无条件调用 `heavy.yml` 的全部重作业）。
- 交付包的 `BUILD-INFO.txt` 记录 `build_time` / `commit` / `protocols` / `sku`，
  现场报问题时先按 `commit` 定位到本文件的对应版本。
- 只有「现场能感知」的变更才写进来：新增协议/端点、行为变化、修复的现场故障、
  交付布局变化、门禁口径变化。纯重构与格式化不记。
- 分类用 `新增 / 变更 / 修复 / 移除 / 安全 / 构建与交付`，同一版本内按影响面排序。

## [Unreleased]

### 新增

- **出厂完整配置模板 `config.factory.json`**（P3-1b）：随包提供一份**完整可加载**的
  `config.json` 初值（示例总线 + 示例点位 + `mqtt` / `web` / `license` 四档）；
  `install.sh` 在包内无现场 `config.json` 时用它兜底落盘，包内 `config.json` 始终优先。
- **Dependabot 依赖升级**（P3-3）：`cargo`（workspace 根）与 `github-actions` 两个生态，
  每周一凌晨跑；minor+patch 合并成一个 PR，major 单独出 PR。

### 变更

- **SQLite schema 版本化**（P2-2）：`PRAGMA user_version` 记录结构版本（当前 v3），
  启动时按 v1→v2→v3 迁移链补齐列与索引；**库版本高于程序版本时拒绝启动**并给出明确报错，
  避免旧程序打开新库导致数据错乱。

### 文档

- 补 P3-5 回填：`docs/linux-branch-review.md` 新增 §10（部署侧 systemd 沙箱与设备白名单），
  `docs/functional-inventory-and-next-roadmap.md` 新增 §1.7（P0-1~P0-4 结论与门禁）；
  两份文档此前完全没有记录「配置落盘被沙箱拒绝」这类只在真机暴露的缺陷。

## [0.1.0] - 2026-11-07

首个交付基线。以「现场装得上、跑得住、出问题查得清」为目标，
包含采集/北向/存储/授权/Web 控制台/OTA 全链路与对应的交付门禁。

### 新增

- **南向采集**：Modbus RTU/TCP、DLT645-2007/1997、CJT188、IEC 60870-5-104、IEC 60870-5-101、
  BACnet MS/TP、OPC UA Client、S7、MC、FINS；统一 `BusManager` 轮询调度、点位级间隔、
  公式引擎、边端处理、采集调优（超时/重试/退避）。
- **存储与队列**：SQLite 落盘队列 + 内存降级队列，断点续传、聚合表、保留天数与容量上限；
  队列计数器与 soak 内存泄漏基线。
- **北向**：MQTT（含 TLS）、OPC UA Server、HTTP/Webhook 通道；通道级重试与断线续传。
- **Web 控制台**（Vue 3 + Element Plus）：配置/点位/设备组/虚拟点/查找替换、协议控制台、
  自检页、OTA、看门狗、证书与审计、License 用量可视化。
- **授权**：License v2 签名校验 + v2.1 配额口径（**配额只管点位**，四档 500/1500/2500/5000，
  代码内置同值默认表 `DEFAULT_TIER_QUOTAS` 兜底）；`license-issuer` 新增 `--sku`，
  未显式给档位时按 SKU 采购推荐矩阵自动填充。
- **安全**：TLS 证书 fail-closed 自举、证书/令牌鉴权、审计日志、配置 `Debug` 脱敏、
  看门狗状态快照 `0600`、配置原子写入创建即 `0600`。
- **OTA**：A/B 分区、pending reboot marker、失败回滚；分区路径参数化
  （`GATEWAY_OTA_SLOT_A/B`，适配 SD / NVMe 介质）。
- **部署**：`deliver.sh` 一键交付（install → 等待就绪 → verify → 交付检查单）、
  `verify-deploy.sh`（自启/环境变量/配置可解析/运行态/授权/时钟）、
  `serial-rules.sh` udev 固定串口名（设备名漂移不再静默断采）、
  musl 静态链接交付包 + 二进制兼容性自检。
- **API**：IEC104 按 IOA 直读端点（未建点位也能读实时值）；`/api/ntp/status` 暴露时钟可信度。

### 变更

- **`gateway-desktop` 冻结**：不再构建/打包/发布，移出 workspace `default-members`
  （代码保留；Tauri 的 Linux 系统依赖不再拖住交付链路）。
- **配额超限语义**：协议种类数与北向通道数超限由**硬拒绝（4021）降级为告警**，
  仅点位数仍硬限制（v2.1 口径）。
- **许可证**：切换为 Apache-2.0，补 `NOTICE`，`SECURITY.md` 改为可公开版本。
- **构建产物目录**收敛到仓库根 `target/`。
- **CI 分层**：重作业（cross-arm / static-musl / coverage / chaos / bench / soak-smoke /
  sku-size-matrix / release-package）抽到 `heavy.yml`，由 `release-gate.yml`
  **无条件**调用（打 tag / 手动 / 每周定时），原先只在手动触发时跑。
- 移除 macOS 交叉平台作业及对应死代码。

### 修复

- **Linux 分支静默失效（第一轮 6 项 + 第二轮 13 项）**：分区并发写导致内容交错、
  错误被吞掉导致「设备看起来正常但已失去心跳/链路/加密保护」等。
- **部署侧 systemd 沙箱（3 项 P0）**：① 配置写回被 `ProtectSystem=strict` + 目录权限拒绝，
  Web 改配置重启即回滚；② `DeviceAllow` 缺 `/dev/gpiochip0`，GPIO/RS485 方向控制 EPERM；
  ③ `DeviceAllow=/dev/ttyUSB`、`/dev/ttyS` 是伪路径，被 systemd 整条丢弃 → 串口采集不可用。
- **SQLite 队列三类计数器漂移**；北向 sink 的 `in_flight` 计数竞态下溢导致 task panic。
- **BACnet MS/TP 帧类型常量**修正为 ASHRAE 135 规定值（原先现场收不到帧）。
- **时钟**：无 RTC / 未同步时的计划任务保护；消除计划任务的静默时区偏移。
- **采集与北向**：Modbus 异常重试死代码；MQTT scheme 误杀；降级队列留存计数误报。
- **启动期行为**：磁盘空间检查与日志保留清理在启动时立即执行（原先前若干秒静默失效）。
- **Web**：`useProtocolCaps` 重试成功后未复位 `failed`；配额超限文字色改用设计令牌。
- **交付脚本**：`verify-deploy.sh` 三处必现误判；覆盖率为 0% 的假失败；
  musl `static-pie` 产物的静态链接误判；交付脚本执行位缺失导致 `Permission denied`。

### 安全

- TLS 证书自举改为 fail-closed，配置不完整时**拒绝启动**而不是静默降级为明文/弱校验。
- 交付与 CI 中不再产生 root 属主 `0600` 的临时文件（原会导致内容正确却断言失败）。

### 构建与交付

- **交付门禁进 CI**：`ci.yml` 的 `deploy-smoke` 真跑 `deploy/build-release.sh`（musl）→
  `deliver.sh` 安装 → 健康度校验 → **通过 Web API 改配置并校验磁盘落盘、重启后仍在**；
  `heavy.yml` 的 `release-package` 用 `cross` 产出 **ARM64 musl 真交付包**，
  逐项断言包内文件、二进制静态链接、`BUILD-INFO.protocols` 与协议注册名一致。
- **`scripts/deploy_lint.ps1` 静态门禁**（121 项）：部署脚本语法、`gateway.service` 的
  `ReadWritePaths` / `DeviceAllow` 与代码实际路径一致性（C9~C12）、CI 必须调用真交付脚本（C16）、
  交付脚本执行位（C17）、工作流变量名笔误（C18）、`sudo cp` 权限陷阱（C19）、
  出厂配置模板完整性（C20）。
- **构建门禁**：ARM 交叉编译（aarch64 / armv7）、ARM 静态 musl 产物、QEMU 运行时验证、
  时区矩阵、SKU 体积观测矩阵、覆盖率地板、chaos 47 场景。

### 已知限制

- `gateway-desktop` 已冻结，不再随版本发布。
- 以下项需真机验收，CI 无法覆盖：udev 稳定名在目标内核上的实际生效、
  OTA A/B 与 U-Boot 槽位定义的同步、无 RTC 设备的时钟兜底行为。
- `bacnet-mstp`、`fins` 处于「退役观察」，不在默认 feature 集内。
