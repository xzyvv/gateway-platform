# Composables 总览

> 路径：`src/composables/`
> 数量：30+ 个 composable（另有 12 个 `.spec.ts` 与实现同目录 co-locate）
> 命名规范：useXxx（驼峰）
> 组织方式：按业务域分 9 个子目录 —— `core` / `auth` / `devices` / `config` / `dashboard` / `data` / `system` / `layout` / `uplink`

## 目录结构（分域速查）

### core/ —— 通用基础设施（错误处理 / 弹窗 / 请求生命周期 / 查询态 / 用户偏好）

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useErrorDisplay | 统一错误显示（toast/inline/modal/silent 四种风格） | 全站 |
| useErrorReporter | 页面级错误上报（对接 PageErrorBoundary / 路由错误处理） | 全站 |
| useUnmountedGuard | 异步组件卸载时取消请求 / 清理定时器 | 多个列表页 / 对话框 |
| useDialogVisible | el-dialog v-model 简化的 ref/emit 包装 | 多个对话框 |
| useQueryState | URL query 与页面筛选态双向同步 | Realtime / History / Logs / ConfigAuditLog / PacketCapture 等 |
| useFavorites | 收藏菜单（用户偏好持久化） | Monitor.vue / useRealtime |
| useDataTableColumns | DataTable 的列显隐过滤 / 表格无障碍名称 / 骨架屏列数 | DataTable.vue |
| useDataTableSelection | DataTable 的勾选态（跨页保留 / 行级禁用 / 全选半选） | DataTable.vue |
| useDataTableVirtual | DataTable 的虚拟滚动引擎选择与列/数据源计算（阈值 200 行） | DataTable.vue |
| useDataTableA11y | DataTable 的无障碍补丁（aria-sort / caption / v2 表格语义兜底） | DataTable.vue |
| useDataTableOverflow | DataTable 的横向溢出提示（右缘渐隐阴影） | DataTable.vue |
| usePerfSampling | FPS / JS Heap 采样（不在挂载时启动，按面板显隐启停，卸载自动停） | RequestMonitor.vue |

### auth/ —— 权限

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| usePermission | 权限检查（角色 / 权限点组合） | 多处（Users 等） |

### devices/ —— 设备与点位

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useDevices | 设备列表 CRUD + 发现对话框（`devices/useDevices/` 目录，barrel 出口） | Devices.vue / DeviceDiscoverDialog.vue |
| useDiscoverWizard | 设备发现向导状态机 | DiscoverWizard.vue |
| usePoints | 点位列表 CRUD + 批量操作（`devices/usePoints/` 目录，barrel 出口） | Points.vue / PointsBatchEditDialog.vue 等 |
| useProtocol | 协议类型元数据 + protocolOptions（单一来源） | ProtocolSelector.vue / Config.vue / 点位相关页 |

### config/ —— 配置

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useConfigActions | 配置编辑动作（保存/导入/导出/预览） | Config.vue |
| useConfigJson | 「原始 JSON」标签文本态与 raw ↔ config 双向同步 | Config.vue / useConfigActions |
| useConfigVersions | 配置版本历史 + diff | ConfigVersions.vue |

### dashboard/ —— 仪表盘

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useDashboardData | 仪表盘汇总数据 + 布局控制 | Dashboard.vue |
| useDashboardLayout | 仪表盘布局设置持久化 | Dashboard.vue |

### data/ —— 数据处理与监控

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useDataFlowTrace | 数据流转追踪可视化 | DataFlowTrace.vue |
| useHistoryQuery | 历史数据查询参数 + 结果 | History.vue |
| useRealtime | 实时数据页面数据订阅（连接 + 表格状态 + 空态分区 + 停滞判定） | RealtimeMonitor.vue |
| useRealtimeChart | 实时趋势图：ECharts 实例 / 100ms 节流重绘 / 历史缓冲 / 跨度标签 | RealtimeMonitor.vue / useRealtime |
| useRealtimeViewport | 实时页视口高度分配（表格 / 图表首屏内布局） | useRealtime |

### system/ —— 系统 / 日志 / 审计 / 会话 / 网络

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useSystem | 网关信息汇总数据（含后端审计日志只读列表） | SystemOverview.vue |
| useLogs | 日志查询 / 过滤 / 归档 | Logs.vue / SystemHub.vue（日志页签） |
| useGatewayDiscovery | 网关局域网发现 | Login.vue / GatewayScanDialog |
| useSessions | 会话 / 连接会话管理 | UplinkTester.vue / SlaveSimulator.vue |

### uplink/ —— 北向 / 上行

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useUplinkConfig | 通道参数视图（多 tab 状态） | UplinkConfig.vue |
| useUplinkTester | 上行测试工具状态机 | UplinkTester.vue |
| useWebhook | Webhook 配置 + 测试 | Ota.vue / useSystem |

### layout/ —— 框架外壳行为（顶栏时钟 / 网关切换）

| 名称 | 职责 | 应用页面 |
|------|------|---------|
| useLayoutClock | 顶栏时钟、网关健康轮询、用户名跨标签同步、主题桥接 | Layout.vue / Topbar.vue |
| useGatewaySwitcher | 多网关切换、LAN 扫描、in-flight 计数 banner | Layout.vue / Topbar.vue / GatewayScanDialog.vue |

### 测试

单元测试与实现同目录 co-locate（`useXxx.spec.ts`），当前已覆盖：useDevices /
usePoints / useSystem / useLogs / useUplinkConfig / useUplinkTester /
useGatewayDiscovery / useProtocol / useLayoutClock / useGatewaySwitcher /
useConfigActions。

## 引用约定

- **跨域引用统一走别名**：`@/composables/<域>/useXxx`（如 `@/composables/system/useSessions`）；
- **同域内引用**可用相对路径 `./useXxx`（如 `devices/usePoints` 引 `./useProtocol`）；
- 引用 composable 之外（api / utils / stores / views / types / mock）一律使用 `@/` 别名，
  避免因子目录层级变化导致相对路径失效；
- 新增 composable 时**先定域、再建文件**：放入对应子目录后按上述约定引用。

## 维护原则

- **业务逻辑放 composables，不放组件**：组件只负责 UI 表达，数据获取/状态机放 composables
- **每个 composable 单一职责**：避免一个 useXxx 包揽"列表 + 编辑 + 弹窗 + 测试"
- **跨页面复用 ≥ 2 次**：抽取为 composables（不要每个页面自己写一份 ref）
- **错误处理统一 useErrorDisplay**：禁止直接 ElMessage.error + alert + console.error 混杂
- **测试与实现同目录**：`.spec.ts` 文件放在对应 composable 同目录

## Store vs Composable 边界（唯一真源）

同一份数据只能有一个所有者，禁止两处各写一套读写逻辑：

| 数据性质 | 归属 | 判定 |
|----------|------|------|
| 跨页面共享、需长期驻留（登录态、网关健康、主题、通知、license 档位、WebSocket 连接） | `src/stores/*.ts`（Pinia） | 多个路由 / 布局组件同时消费 |
| 页面内的一次性数据与交互状态（表单、对话框、分页、筛选） | `src/composables/*/*.ts` | 仅当前页面及其子组件消费 |

- 设备列表当前由 `useDevices` 持有（仅 `Devices.vue` 消费），故不设 store；
  若将来出现 ≥2 个页面需要同一份设备快照，应上移至 `stores/devices.ts` 并让
  composable 委托读取，而不是各页面直接 `listDevices()`。
- 命名冲突时以「谁的生命周期更长」为准：跨路由 → store，随页面创建销毁 → composable。

## 新增 composable 模板

```ts
import { ref, computed } from 'vue'

/**
 * Xxx 职责描述
 *  - 输入: xxx
 *  - 输出: xxx
 *  - 异常: 抛错时如何处理
 */
export function useXxx() {
  const state = ref<Xxx>(initial)
  // ...
  return { state, /* ... */ }
}
```