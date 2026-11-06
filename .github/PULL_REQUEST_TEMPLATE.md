## 改了什么

<!-- 一两句话说明。CI 会在标题里带上 PR 编号，commit message 里不要引用编号。 -->

## 为什么

<!-- 动机比实现更重要。关联 issue：Fixes #123 -->

## 影响面

<!-- 逐项勾选。勾了「是」的那几项请在下面补充说明。 -->

- [ ] 改动了协议能力位（`registry/capabilities.rs`）
- [ ] 新增/删除/变更了 HTTP 端点
- [ ] 改动了配置格式或字段语义
- [ ] 改动了前端页面或页签结构（需同步 `docs/INDEX.md`）
- [ ] 改动了产物体积（需跑 `npm run check:bundle`）
- [ ] 改动了协议帧解析 / 状态机 / 重连退避
- [ ] 仅文档改动

<!--
能力位变更是最容易出事的：声明 true 必须真的有实现，否则
registry/tests.rs 的钉子会挂。若涉及「不适用」而非「未实现」，请在界面上明确区分。

端点变更需同步快照：
    python scripts/gen_api_surface.py
CI 会用 --check 模式校验。
-->

## 验证方式

<!-- 勾选实际跑过的门禁。CI 失败请贴日志。 -->

- [ ] `bash gateway-core/scripts/ci.sh`（fmt / check / clippy / test / build）
- [ ] `cargo test -p gateway-api`（若改了 API）
- [ ] `npm run lint && npm run type-check && npm run test`（若改了前端）
- [ ] `npm run e2e`（若改了页面主流程）
- [ ] `python scripts/gen_api_surface.py --check`（若改了端点）
- [ ] `npm run types:check`（若改了 ts-rs 导出）
- [ ] `pwsh scripts/check_repo_health.ps1`

## 失败模式

<!--
如果改了协议帧解析或状态机，这段必填。
现场网关挂掉一条产线的代价远高于少一个功能，所以要说明：
  - 出错时会发生什么
  - 会不会留下脏状态（半开的连接 / 未回滚的配置 / 卡住的退避计时）
  - 能不能自愈，还是必须人工介入
-->

不涉及则填「不涉及」。

## 文档同步

- [ ] 已更新 `docs/api-surface.md`（如有端点变更）
- [ ] 已更新 `docs/INDEX.md`（如有页签变更）
- [ ] 已更新路线图文档的状态标注（`docs/functional-inventory-and-next-roadmap.md` / `docs/functional-review-and-next-phase.md`）

## 备注

<!-- 已知限制、后续打算、需要的 review 重点。 -->
