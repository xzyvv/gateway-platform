# 工业网关 (Gateway)

工业数据采集网关，支持 Modbus RTU/TCP、DLT645、CJT188 协议，面向工厂表计/PLC 数据采集场景。

> Rust 侧拆分为三个 crate：引擎库 `gateway-core`（本 README 所述配置/采集/北向/队列能力）、Web API 面 `gateway-api`（axum + tower）、可执行 `gatewayd`（网关主程序）。

## 特性

- **多协议采集**：Modbus RTU/TCP（批量合并读）、DLT645（广播校时/事件记录）、CJT188
- **边缘处理**：时间窗口聚合（last/avg/max/min/sum）、死区过滤、变化上传
- **断网续传**：SQLite WAL 持久化队列，MQTT QoS1 ACK 确认
- **工程量换算**：公式/scale+offset 两层换算 + CT/PT 变比独立分层
- **安全**：OTA Ed25519 签名验签、HTTPS、配置敏感字段加密、Bearer Token 鉴权
- **运维**：Web UI（Vue 3 前端，按设备 / 点位 / 采集 / 数据 / 上行 / 运维 / 系统业务域分组，共同构成 70+ 功能页签；协议调试台统一扫描、报文与从站模拟）、Prometheus 指标、JSON 结构化日志、配置版本回滚
- **可靠性**：软件+硬件看门狗、A/B 分区 OTA、supervision 自动重启

## 编译

### 依赖

- Rust 1.70+（edition 2021）
- Linux 交叉编译需要 `gcc-aarch64-linux-gnu`（如目标平台为 ARM64）
- SQLite3（bundled，无需系统安装）

### 编译命令

```bash
# 本机编译（开发/测试，构建网关可执行文件 gatewayd）
cargo build --release -p gatewayd

# 交叉编译 ARM64 Linux（网关硬件平台）
cargo build --release -p gatewayd --target aarch64-unknown-linux-gnu

# 运行测试
cargo test
```

### 编译产物

- `target/release/gatewayd` — 网关主程序（可执行，由 `gatewayd` crate 提供）
- `gateway-api/src/web/builtin_ui.html` — Web UI（编译期嵌入，属 gateway-api）

## 部署

### 目录结构

```
/etc/gateway/
  config.json          # 主配置文件
  templates/builtin/   # 内置设备模板
  templates/custom/    # 用户自定义模板
/var/lib/gateway/
  data_queue.db        # 数据队列（SQLite WAL）
  mqtt_outbox.db       # MQTT QoS1 持久化 outbox
  config_versions.db   # 配置版本快照（SQLite）
  ota/                 # OTA 固件暂存
  certs/               # TLS 证书（自动生成）
/var/log/gateway/
  gateway.log          # 日志（按天滚动，保留7天）
  audit.log            # 操作审计日志
/opt/gatewayd/
  gatewayd              # 主程序二进制
```

### systemd 部署

一键安装（推荐）：

```bash
# 编译
cargo build --release -p gatewayd

# 部署（创建用户、目录、拷贝二进制和配置、安装服务）
sudo bash deploy/install.sh

# 启动
sudo systemctl start gateway
```

手动安装：

```bash
# 创建系统用户
sudo useradd --system --no-create-home --shell /usr/sbin/nologin gateway
sudo usermod -aG dialout gateway

# 创建目录
sudo mkdir -p /opt/gatewayd /etc/gateway/templates/builtin /etc/gateway/templates/custom
sudo mkdir -p /var/lib/gateway/certs /var/lib/gateway/ota /var/log/gateway

# 拷贝二进制和配置
sudo cp target/release/gatewayd /opt/gatewayd/
sudo cp config.json /etc/gateway/
sudo cp -r ../templates/builtin/* /etc/gateway/templates/builtin/

# 设置权限
sudo chown -R gateway:gateway /var/lib/gateway /var/log/gateway

# 安装 systemd 服务
sudo cp deploy/gateway.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable gateway
sudo systemctl start gateway
```

### 配置文件

编辑 `/etc/gateway/config.json`：

```json
{
  "schema_version": 1,
  "buses": [
    {
      "port_name": "/dev/ttyS1",
      "baud_rate": 9600,
      "parity": "even",
      "stop_bits": 1,
      "timeout_ms": 500,
      "protocol": "modbus",
      "modbus_mode": "rtu"
    }
  ],
  "points": [
    {
      "id": "voltage_a",
      "name": "A相电压",
      "bus": "/dev/ttyS1",
      "protocol": "modbus",
      "address": "1",
      "slave_id": 1,
      "address_num": 0,
      "function_code": 3,
      "data_type": "u16",
      "register_count": 1,
      "interval": 1,
      "scale": 0.1,
      "pt_ratio": 100,
      "unit": "V"
    }
  ],
  "mqtt": {
    "enabled": true,
    "broker": "tcp://192.168.1.100:1883",
    "topic": "factory/line1",
    "qos": 1,
    "publish_interval": 60
  },
  "web": {
    "listen_addr": "127.0.0.1:8080",
    "api_token": "your-32-char-token-here!!!",
    "allow_anon_read": false
  }
}
```

> `web.listen_addr` 默认回环（仅本机可访问）。要开放车间 LAN 需改成 `0.0.0.0:8080`，
> **但必须同时设置 `web.password_hash`**，否则触发 fail-closed 拒绝启动。
> 顺序：先保持回环 → 登录后 `POST /api/auth/password` 改密（哈希写回 config.json）
> → 再放开监听地址。

## 现场配置手册

### 1. 选择设备模板

1. 打开 Web UI：浏览器访问 `http://网关IP:8080`
2. 输入 API Token 登录
3. 进入「配置管理」→「简单模式」
4. 从模板下拉中选择电表型号（如"威胜 DTSD342"）
5. 填写电表地址（DLT645 通信地址，12位数字）
6. 修改 CT/PT 变比（电流互感器/电压互感器变比）
7. 点击「应用」

### 2. 配置串口参数

| 参数 | 常用值 | 说明 |
|------|--------|------|
| 波特率 | 4800/9600 | 电表常用 2400-9600 |
| 校验位 | even/none | DLT645 通常 even |
| 停止位 | 1 | |
| 超时 | 500ms | 从站无响应超时 |

### 3. 调试通信

1. 进入「报文调试」tab
2. 选择总线（如 /dev/ttyS1）
3. 观察收发报文：
   - 蓝色 = 网关发出的请求帧
   - 绿色 = 设备返回的响应帧
   - 红色 = 错误/异常响应
4. 如果只有蓝色请求没有绿色响应：
   - 检查接线（A/B 是否接反）
   - 检查终端电阻（120Ω）
   - 检查电表地址是否正确
   - 检查波特率/校验位是否匹配

### 4. 排错指南

| 现象 | 可能原因 | 排查步骤 |
|------|----------|----------|
| 所有点无数据 | 串口配置错误/接线问题 | 看报文调试 tab 是否有响应 |
| 部分点无数据 | 对应电表地址错/离线 | 检查该地址的报文 |
| 数据值异常 | scale/CT/PT 配置错 | 检查工程量换算配置 |
| MQTT 不上送 | broker 不可达 | 看「诊断」tab 的日志 |
| 队列堆积 | 网络断开 | 看「仪表盘」队列深度 |
| 网关重启 | 看门狗超时 | 看「诊断」tab 日志 |

### 5. CT/PT 变比配置

电表读数是二次侧值，需要乘 CT/PT 变比才是一次侧真实值：

- **电压点**：配 `pt_ratio`（如 PT=10000/100，填 100）
- **电流点**：配 `ct_ratio`（如 CT=150/5，填 30）
- **功率/电量点**：配 `ct_ratio` + `pt_ratio`（两个都填）
- **频率/功率因数**：不配变比

### 6. OTA 升级

1. 进入「固件升级」tab
2. 上传固件包（`.bin` 文件）
3. 系统自动验证 → 写入备用分区 → 等待重启
4. 点击「重启」应用新固件
5. 如新固件启动失败，A/B 分区自动回滚

### 7. 敏感字段加密

配置文件中的 `api_token` 和 `mqtt.password` 支持加密存储：

```bash
# 使用网关 CLI 工具加密（在网关上执行）
./gateway encrypt-token "my-secret-token"
# 输出: enc:aBcDeFgHiJkL...

# 将密文写入 config.json
```

加密后的字段以 `enc:` 前缀标识，网关启动时自动解密。密钥绑定机器（CPU ID + machine-id），换机器后无法解密。

## 开发

### 模块结构

```
gateway-core/src/     # 引擎库（lib）
  lib.rs            # crate 入口
  config/           # 配置模型 + 加载/校验/迁移（SQLite 存储见 config/store/）
  south/            # 南向采集：总线管理 / 协议栈 / 调度器
  pipeline/         # 边缘处理管道（过滤/公式/入队）
  queue/            # SQLite WAL 队列
  north/            # 北向上报（MQTT/Webhook/Server）
  platform/         # NTP / OTA / GPIO / 蜂窝 / 看门狗
  engine/           # 日志 / 指标 / 事件总线 / 健康监控
  crypto/ license/  # 加密与 license 校验

gateway-api/src/web/  # Web API 面（axum + tower）：REST/WS handler + 内置 UI
gatewayd/src/         # bin：main.rs 入口 + bootstrap 启动编排
```

### 测试

```bash
cargo test
cargo clippy
cargo fmt --check
```

## License

Proprietary - All rights reserved.
