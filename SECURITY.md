# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |
| < 0.1   | :x:                |

## Reporting a Vulnerability

工业网关稳定性优先，请严肃对待安全漏洞。

**首选渠道：GitHub 私密漏洞上报**
在仓库的 **Security → Advisories → Report a vulnerability** 提交报告。
该渠道内容仅项目维护者可见，报告会生成一个私密的 advisory 页面，
可与报告者持续沟通修复进展，且不会留下公开的时间戳痕迹。

**备选渠道：邮箱**
- **邮箱**：3564539583@qq.com
- **响应时长**：3 个工作日内确认收到，7 个工作日内给出评估结论

**请不要**在公开 Issue / PR / 微信群中直接披露未修复漏洞。
若仓库尚未开启 GitHub 私密上报（Settings → Security → General），
请直接走邮箱渠道。

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

## 许可与授权相关的安全问题

本项目开源代码采用 Apache-2.0，但**官方发行版的授权（license）签发是另一回事**。
以下情况请按上面的渠道私密上报，不要公开讨论：

| 情形 | 说明 |
|---|---|
| 签发私钥泄露 | `tools/license-issuer/keys/license_ed25519.pk8` 泄露等同于他人可签发任意授权 |
| 授权绕过 | 找到无需有效 license 即可突破配额校验的路径 |
| 指纹伪造 | 能构造出通过机器指纹绑机校验的伪造标识 |
| 授权文件篡改生效 | 能让被篡改的 license 通过验签 |

签名与验签算法、机器指纹的计算口径都在公开源码里（这是设计如此，
让社区能自签发自用），**安全性依赖的是私钥不外泄**，不依赖算法保密。

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
