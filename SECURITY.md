# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |
| < 0.1   | :x:                |

## Reporting a Vulnerability

工业网关稳定性优先，请严肃对待安全漏洞。

- **邮箱**：3564539583@qq.com
- **响应时长**：3 个工作日内确认收到，7 个工作日内给出评估结论
- **请不要**在公开 Issue / PR / 微信群中直接披露未修复漏洞

报告内容请包含：
1. 受影响版本与文件/模块路径
2. 复现步骤（最小化 PoC 优先）
3. 影响评估（远程利用 / 本地利用 / DoS / 数据泄露等）
4. 建议的修复方向（可选）

## 处理流程

1. 接收确认 → 评估严重性（Critical / High / Medium / Low）
2. Critical / High：72 小时内出临时缓解方案，7 天内出修复版本
3. Medium / Low：下个版本修复
4. 修复版本发布后 14 天披露细节（与报告者协商）

## 供应链安全

本项目通过 CI 自动执行：
- `cargo audit`：扫描 RustSec 公告库
- `cargo deny check`：许可证合规 + ban crate + 来源校验

提交 PR 前请本地运行：

```bash
cargo audit
cargo deny check
```

## 鉴权与配置安全

- `web.api_token` 必须 >= 16 字符，空值时启动失败（fail-closed）
- TLS 证书首次启动自动生成自签名证书，存放于 `/var/lib/gateway/certs/`
- 配置历史快照存于 `config_versions.db`，便于追溯变更
