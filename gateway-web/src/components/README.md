# Components 总览

> 路径：`src/components/`

按职责分为 3 类：工业表单输入控件、展示控件、容器/路由控件。

## 目录结构

```
components/
├── industrial/                 # 工业表单输入控件集（详见 industrial/README.md）
│   ├── BaudrateSelect.vue
│   ├── IntervalSelect.vue
│   ├── DurationSelect.vue
│   ├── SlaveIdInput.vue
│   ├── ProtocolSelector.vue
│   └── README.md
│
├── dataTable/                  # DataTable 的内部子组件（页面不直接引用，见下表）
│   ├── DataTablePlain.vue      # 普通表格分支（排序 / 固定列 / 溢出提示）
│   ├── DataTableVirtual.vue    # 虚拟滚动分支（超阈值时启用，按需异步加载）
│   ├── DataTablePager.vue      # 分页器
│   ├── DataTableErrorBar.vue    # 加载失败错误条 + 重试
│   ├── cellView.ts             # 单元格渲染器（继承使用方 scopeId）
│   └── types.ts                # ColumnDef 列定义类型
│
├── DataTable.vue                # 全站统一表格封装（普通表 / 虚拟滚动双引擎）
├── StatCard.vue                # 统计卡片（标签 + 数值 + 进度条）
├── SectionHeader.vue            # 区块标题（图标 + extra 操作）
├── EmptyState.vue              # 空状态（基于 el-empty）
├── FreshnessBadge.vue           # 数据新鲜度标签（5s/30s/超时）
├── HubShell.vue                # 聚合页外壳（tabs + tier + URL 同步）
├── PageErrorBoundary.vue        # 页面级错误边界（按路由粒度降级）
└── RequestMonitor.vue           # 请求监控（HTTP 状态展示）
```

## 分类速查

### 1. 工业表单输入控件集（`industrial/`）

参见 [`industrial/README.md`](./industrial/README.md)。10 个组件，统一封装工业常用值集合，避免每个表单重复维护 el-option v-for。仅支持按文件引入（无 `index.ts` 桶）。

### 2. 展示控件

| 组件 | 职责 | 应用页面 |
|------|------|---------|
| **StatCard** | 数字 + 进度条 + 图标统一卡片 | Dashboard / MetricsDashboard / MultiUplinkStatus / History 等 |
| **SectionHeader** | el-card 标题块（带 icon + extra 操作位） | 所有页面卡片头部 |
| **EmptyState** | 表格/列表空态（基于 el-empty） | 所有表格 `#empty` slot |
| **FreshnessBadge** | 时间戳新鲜度（fresh/warn/stale） | 实时数据表格 |
| **DataTable** | 全站统一表格封装（普通表 / 虚拟滚动双引擎 + 骨架 / 空态 / 分页 / 批量条） | 全站所有列表页 |
| **DataTablePlain** | DataTable 的普通表格分支（排序 / 固定列 / 溢出提示 / 选择列），内部组件，页面不直接引用 | DataTable 内 |
| **DataTableVirtual** | DataTable 的虚拟滚动分支（超阈值时按需异步加载组件与样式），内部组件，页面不直接引用 | DataTable 内 |
| **DataTablePager** | DataTable 下沉的分页器，内部组件，页面不直接引用 | DataTable 内 |
| **DataTableErrorBar** | DataTable 下沉的错误条 + 重试按钮，内部组件，页面不直接引用 | DataTable 内 |

### 3. 容器 / 路由控件

| 组件 | 职责 | 替代方案 |
|------|------|---------|
| **HubShell** | 7 个 Hub 页的公共外壳（tabs/tier/URL 三件套） | 每个 Hub 退化为"声明 tabs 数组"的薄壳 |
| **PageErrorBoundary** | 页面级错误边界：onErrorCaptured 拦截 + 降级卡片（重试/刷新整页/复制错误）+ reporter 上报 | 全站唯一错误边界（旧 `ErrorBoundary` 已按 N-9 删除） |
| **RequestMonitor** | HTTP 请求状态实时展示（侧边或顶部） | 无 |

## 维护原则

- **新组件先分类**：判断属于"工业表单/展示/容器"哪一类
- **工业表单优先复用 `industrial/`**：不要在 views/ 下复制已有控件
- **跨页面重复 ≥ 3 次**：抽取到 components/，避免在多个 views/ 内重复实现
- **组件内部不写业务逻辑**：数据获取放 composables/，UI 表现放 components/
- **JSDoc 注释必填**：每个组件头部写明 props / emits / 典型应用场景

## 展示型组件范式（W2-#11 收口）

同一类展示形态只允许一条实现路径，页面不要再手写替身：

| 场景 | 唯一实现 | 禁止 |
|------|---------|------|
| 列表/表格空态 | `EmptyState.vue`（带 `action-text` 时自动给 CTA） | 页面直接写 `<el-empty>` |
| 统计卡片（标签 + 数值 + 进度） | `StatCard.vue` | 页面手写 `class="stat-card"` 卡片 |
| 筛选条（下拉 + 输入 + 查询/重置） | `FilterToolbar.vue` | 每个页面各拼一套 `el-select` + `el-input` 行 |
| 表格加载骨架 | `DataTable` 内置的 `TableSkeleton.vue` | 在表格位置手写 `el-skeleton` 行占位 |

上述四条均由 `uiConsistency.spec.ts` 静态扫描守护，新增手写实现会直接让单测失败。

例外口径：**只有表格位置**必须走 `TableSkeleton`。卡片、图表、代码块、diff 预览等非表格位置的加载占位，允许页面直接写 `el-skeleton`（形态各有差异，抽公共组件反而增加耦合）。
