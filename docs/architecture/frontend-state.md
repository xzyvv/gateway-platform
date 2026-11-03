# 前端状态管理边界：Store vs Composable

## 核心原则

| 维度 | Pinia Store | Composable |
|------|-------------|------------|
| **生命周期** | 全局单例，应用存活期间持久 | 跟随调用组件的 setup，组件卸载即销毁 |
| **共享范围** | 跨页面 / 跨组件共享 | 单页面内部 / 组件局部 |
| **典型状态** | 用户认证、网关配置、设备列表、全局通知 | 表单草稿、筛选条件、分页、对话框开关 |
| **是否持久化** | 可持久化（localStorage） | 不持久化 |
| **测试** | 独立测试 store 逻辑 | 随组件测试 |

## 判断规则（按顺序检查）

1. **是否需要跨页面共享？** → 是 → Store
   - 例：登录态（auth）、网关配置（config）、设备树（devices）
2. **是否需要在多个不相关组件间共享？** → 是 → Store
   - 例：全局通知队列（notifications）、网关连接状态（gateway）
3. **是否仅在单个页面/视图内使用？** → 是 → Composable
   - 例：实时数据页的筛选条件、历史查询页的时间范围、日志页的级别过滤
4. **是否是可复用的 UI 行为（与具体业务无关）？** → 是 → Composable
   - 例：对话框开关（useDialogVisible）、卸载守卫（useUnmountedGuard）

## 现有 Store 清单

| Store | 职责 | 跨页面共享 |
|-------|------|-----------|
| `auth` | 登录态、token、权限 | ✅ |
| `config` | 网关配置（buses/points/devices） | ✅ |
| `devices` | 设备树/点位列表缓存 | ✅ |
| `gateway` | 网关连接状态、版本信息 | ✅ |
| `notifications` | 全局通知队列 | ✅ |

## 典型 Composable 分类

**页面级状态**（不跨页）：
- `useRealtime` — 实时数据页筛选
- `useSystem` — 系统页状态
- `useHistoryQuery` — 历史查询条件
- `useLogs` — 日志页过滤

**可复用行为**（无业务状态）：
- `useDialogVisible` — 对话框开关
- `useUnmountedGuard` — 卸载后状态更新守卫
- `usePermission` — 权限检查

**业务封装**（调用 API + 局部状态）：
- `useConfigActions` — 配置增删改（操作 config store）
- `usePoints` / `useDevices` — 点位/设备操作
- `useWebhook` — Webhook 配置

## @singleton 标注（有意单例）

以下 composable 使用模块级 `ref` 创建全局单例，是有意为之的设计决策。
每个都标注了 `// @singleton` 注释和 `// eslint-disable-next-line no-restricted-syntax`，
ESLint 规则会 warn 新增的模块级 ref，防止无意中引入新的全局单例。

| Composable | 模块级 ref | 原因 |
|------------|-----------|------|
| `useFavorites` | `favorites` | 收藏列表跨页面共享（Realtime + 列表页），localStorage 持久化 |
| `useDashboardLayout` | `sections` | 仪表盘布局用户偏好，跨组件共享，localStorage 持久化 |
| `useSessions` | `sessions` | 登录会话列表跨页面共享（路由守卫 + 管理页），localStorage 持久化 |
| `useRequestLog` | `logs`, `paused` | 全局请求日志，拦截器写入 + 面板读取，不绑定页面生命周期 |
| `useWebhook` | `webhookUrl`, `deadLetterQueue` | Webhook 配置跨页面共享（设置页 + 通知触发），localStorage 持久化 |

> 后续如需 DevTools 追踪或 SSR 兼容，可逐步迁移到 Pinia store。

## 反模式（避免）

- ❌ 在 composable 中管理应该全局共享的状态（如 token）
- ❌ 在 store 中放仅单页用的临时状态（如表单草稿）
- ❌ composable 之间直接共享 mutable 状态（应通过 store 中转）
- ❌ store 中包含 UI 交互逻辑（应放 composable）

## 新增状态时的决策流程

```
需要持久化或跨页面共享?
  ├─ 是 → Pinia Store（src/stores/）
  └─ 否 → 仅当前页面用?
           ├─ 是 → 页面内 ref / 页面级 composable
           └─ 否 → 通用行为 → 可复用 composable（src/composables/）
```
