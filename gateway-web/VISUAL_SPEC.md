# 工业网关 Web 控制台 — 视觉规范文档

> 版本：v1.1 | 更新日期：2026-10-26
> 技术栈：Vue 3 + Vite + Element Plus + ECharts
> 适用范围：`gateway-web/src/` 全部页面与组件
> v1.1 变更：品牌主色 #409eff → 靛蓝 #4f46e5（含 EP 变量族映射与图表主题）；新增 §11 登录页视觉规范

---

## 目录

1. [设计理念](#1-设计理念)
2. [色彩系统](#2-色彩系统)
3. [渐变装饰语言](#3-渐变装饰语言)
4. [圆角规范](#4-圆角规范)
5. [阴影体系](#5-阴影体系)
6. [间距规范](#6-间距规范)
7. [动效与过渡](#7-动效与过渡)
8. [组件视觉规范](#8-组件视觉规范)
9. [深色代码区域规范](#9-深色代码区域规范)
10. [响应式断点](#10-响应式断点)
11. [登录页](#11-登录页)

---

## 1. 设计理念

- **统一优先**：所有页面遵循同一套设计令牌（CSS Variables），禁止硬编码颜色/圆角/阴影
- **语义驱动**：颜色按功能语义划分（主色/成功/警告/危险/信息），不随页面变化
- **克制装饰**：装饰元素（渐变条、光晕）仅出现在关键视觉锚点，不滥用
- **反馈明确**：每个交互态（hover/active/focus/disabled）都有可感知的视觉反馈

---

## 2. 色彩系统

### 2.1 主色调

品牌主色为**靛蓝 Indigo**（v1.1 起，替换原 Element Plus 出厂蓝 #409eff，衍生阶对应 Tailwind indigo 色阶）：

| Token | 亮色值 | 用途 |
|-------|--------|------|
| `--color-primary` | `#4f46e5`（indigo-600） | 品牌主色，按钮/链接/激活态/图标 |
| `--color-primary-light` | `#818cf8`（indigo-400） | hover 态主色 |
| `--color-primary-lighter` | `#a5b4fc`（indigo-300） | 浅主色（选中/降级强调） |
| `--color-primary-lightest` | `#e0e7ff`（indigo-100） | hover 背景 |
| `--color-primary-bg` | `#eef2ff`（indigo-50） | 主色背景（选中行/标签） |
| `--color-primary-border` | `#c7d2fe`（indigo-200） | 主色边框 |
| `--color-primary-shadow` | `rgba(79,70,229,0.3)` | 主色投影 |

**暗色规则**：`html.dark` 下整个家族重声明为 indigo-400 系（`--color-primary: #818cf8`、lightest/border 转深底融合色 `#272a4a`/`#41467c`），light-N 槽位经 `var()` 自动跟随。EP 组件变量族在 `:root:root` 统一映射到项目 token（见 global.css），禁止在业务代码直接引用 `--el-color-primary*`。

### 2.2 功能色

| Token | 值 | 用途 |
|-------|-----|------|
| `--color-success` | `#67c23a` | 成功（在线/正常/完成）——**仅限图标/边框/进度条填充等非文字场景** |
| `--color-warning` | `#e6a23c` | 警告（积压/待处理）——同上 |
| `--color-danger` | `#f56c6c` | 危险（错误/异常/删除）——同上 |
| `--color-info` | `#909399` | 信息（次要/中性） |
| `--color-purple` | `#9c27b0` | 指标卡紫色变体（StatCard stat-purple 的 icon/进度条，明暗通用） |

> 原色直接当文字色在白底上仅 2.04~2.90:1（WCAG AA 需 4.5）。**文字场景一律用 §2.3 的 `--text-success/warning/danger`**；暗色主题下三者经 `var()` 自动切到提亮的 `-light` 令牌。

### 2.3 文字色

| Token | 值 | 用途 |
|-------|-----|------|
| `--text-primary` | `#303133` | 标题/关键数据 |
| `--text-regular` | `#606266` | 正文/标签 |
| `--text-secondary` | `#646b77` | 次要信息/辅助说明（由 `#909399` 调深；页面底/语义底均 ≥4.79:1，满足 WCAG AA；暗色 `#aab0bb`） |
| `--text-placeholder` | `#676d78` | 占位符（由 `#6f7581` 再调深；各底色均 ≥4.64:1，厂区强光下可辨认；暗色 `#a3a8b3`） |
| `--text-disabled` | `#a8abb2` | 禁用文字 |
| `--text-success` | `#357a1d` | 语义色文字（白底 5.31:1；暗色自动切 `--color-success-light`） |
| `--text-warning` | `#92400e` | 语义色文字（白底 7.09:1；暗色自动切 `--color-warning-light`） |
| `--text-danger` | `#b91c1c` | 语义色文字（白底 6.47:1；暗色自动切 `--color-danger-light`） |

### 2.4 边框色

| Token | 值 | 用途 |
|-------|-----|------|
| `--border-base` | `#ebeef5` | 卡片/表格边框 |
| `--border-light` | `#e4e7ed` | 浅色分割线 |
| `--border-lighter` | `#dcdfe6` | 输入框边框 |
| `--border-extralight` | `#f0f2f5` | 极浅分割线 |

### 2.5 背景色

| Token | 值 | 用途 |
|-------|-----|------|
| `--bg-page` | `#f0f2f5` | 页面背景 |
| `--bg-card` | `#ffffff` | 卡片背景 |
| `--bg-card-header` | `#fafafa` | 卡片头部/斑马纹偶数行 |
| `--bg-fill` | `#f5f7fa` | 填充背景（表头/禁用） |

### 2.6 半透明叠加与反白

品牌色/功能色的半透明叠加统一写 `rgba(var(--*-rgb), α)`，不再散落 `rgba(64,158,255,…)` 之类字面量；
品牌色、深色底上的白字统一用 `--text-inverse`。

| Token | 值 | 用途 |
|-------|-----|------|
| `--color-primary-rgb` | `79, 70, 229` | 主色通道（暗色下重声明为 `129, 140, 248`） |
| `--color-success-rgb` | `103, 194, 58` | 成功色通道 |
| `--color-warning-rgb` | `230, 162, 60` | 警告色通道 |
| `--color-danger-rgb` | `245, 108, 108` | 危险色通道 |
| `--color-info-rgb` | `144, 147, 153` | 信息色通道 |
| `--color-accent-rgb` | `54, 207, 201` | 青色通道 |
| `--dark-accent-rgb` | `79, 195, 247` | 深色区青色通道 |
| `--text-inverse` | `#ffffff` | 品牌色/深色底上的反白文字与前景 |

---

## 3. 渐变装饰语言

> **核心规范**：全系统所有渐变装饰条统一使用 `linear-gradient(90deg, var(--color-primary) 0%, var(--color-accent) 100%)`，禁止使用其他渐变色。

### 3.1 渐变定义

```css
--gradient-brand: linear-gradient(90deg, var(--color-primary) 0%, var(--color-accent) 100%);
```

- **方向**：从左到右（90deg），大面积填充用 135deg
- **起始色**：品牌靛蓝（亮色渲染 `#4f46e5`，暗色 `#818cf8`，由 token 主题自动切换）
- **结束色**：青色 `#36cfc9`（`--color-accent`，两主题通用）
- **用途**：仅用于装饰性条/线与登录页品牌锚点，不用于数据区域大面积填充

### 3.2 应用场景清单

| 场景 | 位置 | 尺寸 | 说明 |
|------|------|------|------|
| 区块标题竖条 | 各页面 `.section-header::before` / `.section-title::before` | 4×14~16px | 标题左侧装饰 |
| 深色代码区顶部条 | Config/Logs/System 深色区域 `::before` | 100%×3px | sticky 定位，滚动可见 |
| 对话框顶部条 | `.el-dialog__header::before` | 100%×3px | 与对话框圆角对齐 |
| 登录页品牌锚点 ×4 | 见 §11.2 | 3px 条 / 72px 块 / 28px 线 / 按钮填充 | 卡片顶条、Logo 块、标题短线、登录按钮 |
| Tabs 激活指示条 | `.el-tabs__active-bar` | 100%×3px | 带蓝色光晕阴影 |

### 3.3 辅助渐变色（指标卡专用）

仅 Dashboard 指标卡使用差异化渐变，增强数据辨识度：

| 指标 | 渐变 |
|------|------|
| 在线设备 | `linear-gradient(135deg, #667eea 0%, #764ba2 100%)` |
| 队列积压 | `linear-gradient(135deg, #f093fb 0%, #f5576c 100%)` |
| 内存使用率 | `linear-gradient(135deg, #4facfe 0%, #00f2fe 100%)` |

---

## 4. 圆角规范

| Token | 值 | 适用组件 |
|-------|-----|---------|
| `--radius-dialog` | `14px` | 对话框 |
| `--radius-card` | `10px` | 卡片 |
| `--radius-picker` | `10px` | 日期选择器面板 |
| `--radius-table` | `8px` | 表格容器 |
| `--radius-input` | `8px` | 输入框 |
| `--radius-button` | `6px` | 按钮 |
| `--radius-tag` | `6px` | 标签 |
| `--radius-tag-round` | `10px` | 圆角标签 |
| `--radius-section-icon` | `8px` | 区块标题图标容器 |
| `--radius-popper` | `6px` | Tooltip/弹层 |
| `--radius-checkbox` | `4px` | 复选框 |
| `--radius-small` | `4px` | 小型元素 |
| `--radius-bar` | `2px` | 区块标题渐变装饰竖条（4×16px，§3.2） |

---

## 5. 阴影体系

| Token | 值 | 用途 |
|-------|-----|------|
| `--shadow-card` | `0 1px 4px rgba(0,0,0,0.06)` | 卡片默认 |
| `--shadow-card-hover` | `0 4px 16px rgba(0,0,0,0.1)` | 卡片悬浮（基础） |
| `--shadow-dialog` | `0 20px 60px rgba(0,0,0,0.15)` | 对话框 |
| `--shadow-popper` | `0 4px 12px rgba(0,0,0,0.15)` | Tooltip/弹层 |
| `--shadow-message` | `0 8px 24px rgba(0,0,0,0.12)` | 消息/通知 |
| `--shadow-dropdown` | `0 6px 24px rgba(0,0,0,0.12)` | 下拉菜单 |
| `--shadow-picker` | `0 8px 28px rgba(0,0,0,0.12)` | 日期选择器 |

### 卡片悬浮增强

卡片 hover 时叠加主色光晕环（用主色通道变量，不用旧 EP 蓝字面量）：
```css
box-shadow: 0 8px 24px rgba(0,0,0,0.08), 0 0 0 1px rgba(var(--color-primary-rgb), 0.15);
border-color: rgba(var(--color-primary-rgb), 0.3);
transform: translateY(-2px);
```

---

## 6. 间距规范

| Token | 值 | 用途 |
|-------|-----|------|
| `--spacing-card-gap` | `16px` | 卡片之间间距 |
| `--spacing-card-body` | `20px` | 卡片内容区内边距 |
| `--spacing-card-header-y` | `16px` | 卡片头部上下内边距 |
| `--spacing-card-header-x` | `20px` | 卡片头部左右内边距 |
| `--spacing-form-item` | `20px` | 表单项之间间距 |
| `--spacing-table-cell` | `10px 0` | 表格单元格垂直内边距 |

### 表格首末列

```css
.el-table .el-table__cell:first-child .cell { padding-left: 16px; }
.el-table .el-table__cell:last-child .cell { padding-right: 16px; }
```

### 字号规范

- 业务样式中的字号统一使用 **rem**（基准 `1rem = 16px`），禁止写死 `px` 字号
- 根字号保持浏览器默认 16px；把 `html` 字号调到 32px 即等价于全站 200% 字号缩放
- Element Plus 字号令牌已映射为 rem（`--el-font-size-*`，见 `global.css` 的 `:root:root`）
- 例外：ECharts canvas 的 `fontSize` 是数值像素，无法使用 rem

---

## 7. 动效与过渡

### 7.1 过渡时长

| Token | 值 | 用途 |
|-------|-----|------|
| `--transition-fast` | `0.2s ease` | 按钮/标签/输入框 hover |
| `--transition-medium` | `0.25s ease` | 卡片/页面过渡 |
| `--transition-slow` | `0.3s ease` | 开关/进度条 |
| `--transition-card-enter` | `0.4s ease-out` | 卡片入场动画 |

### 7.2 关键动画

| 动画名 | 时长 | 曲线 | 用途 |
|--------|------|------|------|
| `cardEnter` | 0.4s | ease-out | 卡片入场（opacity + translateY） |
| `errorSlideIn` | 0.3s | ease-out | 表单错误提示滑入 |
| `pulse` | 2s | ease-in-out infinite | 实时状态点脉冲 |
| `breathing` | 3s | ease-in-out infinite | Logo 呼吸发光 |
| `float` | 8~12s | ease-in-out infinite | 登录页光球浮动 |
| `loading-rotate` | 1.2s | linear infinite | 加载旋转 |

### 7.3 页面路由过渡

```css
.fade-enter-from { opacity: 0; transform: translateY(10px); }
.fade-leave-to { opacity: 0; transform: translateY(-10px); }
```

---

## 8. 组件视觉规范

### 8.1 按钮

| 属性 | 规范 |
|------|------|
| 圆角 | `--radius-button` (6px) |
| 字重 | 500 |
| hover | `translateY(-1px)` + 主题色光晕阴影 |
| active | `translateY(0) scale(0.98)` |
| 图标间距 | 6px |
| 主色按钮 hover 阴影 | `0 4px 12px var(--color-primary-shadow)` |
| 成功按钮 hover 阴影 | `0 4px 12px rgba(103,194,58,0.3)` |
| 警告按钮 hover 阴影 | `0 4px 12px rgba(230,162,60,0.3)` |
| 危险按钮 hover 阴影 | `0 4px 12px rgba(245,108,108,0.3)` |

### 8.2 卡片

| 属性 | 规范 |
|------|------|
| 圆角 | `--radius-card` (10px) |
| 边框 | 透明（hover 时变蓝） |
| 默认阴影 | `--shadow-card` |
| 入场动画 | `cardEnter` 0.4s |
| hover | 上浮 2px + 蓝色光晕环 + 蓝色边框 |
| 头部背景 | `--bg-card-header`（hover 时） |

### 8.3 表格

| 属性 | 规范 |
|------|------|
| 圆角 | `--radius-table` (8px) |
| 边框 | 1px `--border-base` |
| 表头背景 | `linear-gradient(180deg, #fafbfc, #f5f7fa)` |
| 表头顶部边框 | 2px `--color-primary` |
| 斑马纹 | 偶数行 `--bg-card-header` |
| 行 hover | 浅蓝背景 `--color-primary-lightest` + 首列左侧 3px 蓝条 |
| 单元格内边距 | `10px 0`，首列左 16px，末列右 16px |
| 排序激活色 | `--color-primary` |

### 8.4 描述列表

| 属性 | 规范 |
|------|------|
| label 背景 | `--bg-card-header` |
| label 字重 | 600 |
| 首行顶部边框 | 2px `--color-primary` |
| 行 hover | label `#f0f7ff`，content `#fafdff` |
| 内容换行 | `word-break: break-all; overflow-wrap: break-word;` |

### 8.5 输入框

| 属性 | 规范 |
|------|------|
| 圆角 | `--radius-input` (8px) |
| 聚焦 | `box-shadow: 0 0 0 1px var(--color-primary) inset` + 外扩光晕 |
| 前缀图标聚焦 | 变为主色 |
| 后缀图标 hover | 变为主色 |

### 8.6 标签 Tag

| 属性 | 规范 |
|------|------|
| 边框 | 无 |
| 字重 | 500 |
| hover（非可关闭） | `brightness(0.96)` + 上浮 1px |
| 关闭按钮 | 16px 圆形，hover 半透明黑背景 |

### 8.7 Tabs

| 属性 | 规范 |
|------|------|
| 激活项字重 | 600 |
| hover | 主色文字 |
| 激活指示条 | 蓝绿渐变 + 蓝色光晕 + 弹性过渡 |
| 底部线 | 1px `--border-extralight` |

### 8.8 反馈组件

| 组件 | 规范 |
|------|------|
| **Notification** | 10px 圆角 + 左侧 3px 语义色条 + 柔和阴影 |
| **Message** | 8px 圆角 + 按类型着色背景 |
| **Alert** | 左侧 3px 语义色条（info灰/success绿/warning橙/error红） |
| **表单错误** | 红色圆形警告图标 + 0.3s 滑入动画 |
| **空状态** | 40px 上下内边距 + 图片 0.7 透明度（hover 1.0） |

### 8.9 日期选择器

| 属性 | 规范 |
|------|------|
| 面板圆角 | `--radius-picker` (10px) |
| 今天标识 | 蓝色文字 + 底部 4px 蓝色圆点 |
| 选中态 | 蓝色填充背景 + 白色文字 |
| hover | 浅蓝背景 + 蓝色文字 |

---

## 9. 深色代码区域规范

> 全系统 4 处深色代码展示区域遵循统一规范。

### 9.1 通用样式

```css
background: var(--code-bg-alt);
border: 1px solid var(--code-border);
border-radius: 8px;
font-family: 'Consolas', 'Monaco', monospace;
color: var(--code-text);
```

> 代码区已令牌化：背景 `--code-bg` / `--code-bg-alt`、面板 `--code-panel`、边框 `--code-border` / `--code-border-alt`、
> 正文 `--code-text`、语法高亮 `--code-key` / `--code-string` / `--code-number` / `--code-boolean` / `--code-attr` /
> `--code-muted` / `--code-muted-border` / `--code-danger` / `--code-warning`、滚动条 `--code-scrollbar` / `--code-scrollbar-hover`。

### 9.2 顶部装饰条

```css
position: sticky;  /* 或 fixed */
top: 0;
height: 3px;
background: linear-gradient(90deg, var(--color-primary) 0%, var(--color-accent) 100%);
```

### 9.3 应用场景

| 场景 | 页面 | 装饰条定位 |
|------|------|-----------|
| 原始 JSON 编辑器 | Config | fixed |
| 日志查看框 | Logs | sticky |
| 命令输出框 | Logs | sticky |
| 系统指标 JSON | System | sticky |

---

## 10. 响应式断点

| 断点 | 宽度 | 适配策略 |
|------|------|---------|
| 桌面端 | ≥ 1200px | 全量布局 |
| 平板端 | 768px ~ 1199px | 侧边栏可折叠，指标卡 2 列 |
| 移动端 | < 768px | 侧边栏隐藏，顶栏面包屑隐藏，单列布局 |

---

## 11. 登录页

> 实现文件：`src/views/auth/Login.vue` + `Login.css`（独立样式文件，scoped 引入）

### 11.1 布局与背景

居中单卡布局（400px，max-width 90vw），背景三层装饰叠加，全部 `pointer-events: none` 且**纯静态**（无任何常驻动画）：

| 装饰层 | 实现 | 亮色 | 暗色 |
|--------|------|------|------|
| 网格纹理 | `.login-container::before`，40px 方格 | `rgba(var(--color-primary-rgb), 0.06)` | `rgba(var(--dark-accent-rgb), 0.04)` |
| 装饰圆 ×3 | `.login-bg-shape.shape-1/2/3` | `rgba(var(--color-primary-rgb), 0.08)`，shape-3 用 accent 通道 | 同亮色（半透明两主题通用） |
| 径向光晕 | `.login-container::after`，600px 居中 | `rgba(var(--color-primary-rgb), 0.1)` | `rgba(var(--dark-accent-rgb), 0.1)` |

页面底色随主题：亮色为浅靛蓝渐变 `var(--color-primary-bg) → var(--bg-page) → var(--color-primary-bg)`（135deg）；暗色在 `html.dark` 中整体覆盖为深藏青 `--dark-bg-deep → --dark-bg → --dark-bg-deeper` 渐变。

### 11.2 品牌锚点（靛蓝→青渐变 ×4）

四处锚点复用同一品牌渐变（§3.1），构成登录页的全部品牌识别：

| 元素 | 选择器 | 规格 |
|------|--------|------|
| 卡片顶部渐变条 | `.login-card::before` | 3px 全宽，与表头/对话框顶条同语言，圆角随卡片 16px |
| Logo 渐变块 | `.login-logo` | 72×72px，圆角 20px，内含 `Cpu` 芯片图标（工业网关语义）36px 反白，投影 `rgba(var(--color-primary-rgb), 0.4)` |
| 标题装饰短线 | `.login-header h1::after` | 28×3px 居中；h1 本体为实色 `var(--text-primary)` 深字（不用背景渐变 clip 文字） |
| 登录按钮 | `.login-btn` | 135deg 渐变填充、8px 圆角、hover 上浮 1px + 主色投影 |

副标题「直连调试控制台」保持 `--text-secondary` + 4px 字距，不参与渐变。

### 11.3 交互态

- 输入聚焦环：`0 0 0 1px var(--color-primary) inset, 0 0 0 4px rgba(var(--color-primary-rgb), 0.15)`；聚焦态输入文字亮色 `--text-primary` / 暗色 `--el-text-color-primary`
- 按钮焦点可见环：`outline-color: var(--color-primary)` + 4px 主色光晕
- 网关地址连通提示：图标变 `--color-success` 并以 `gatewayPulse 2s ease-in-out 3` 呼吸 **3 次后静止**
- 局域网发现列表：候选项 hover/选中用 `rgba(var(--color-primary-rgb), 0.12/0.2)` 底色，焦点环 `outline: 2px solid var(--color-primary)`

### 11.4 动效与资源约束

- 唯一入场动画：卡片 `cardIn 0.5s ease`（20px 上移淡入），背景装饰层禁止动画
- **零图片资源**：Logo 用图标字体 + CSS 渐变实现，不引入 SVG/位图
- 禁止硬编码品牌 hex（历史遗留的 `--dark-accent` cyan 写法已全部迁移至 `var(--color-primary)` 系列）

### 11.5 验证记录（2026-10-26）

`vite build` 通过；vitest 115 文件 / 937 用例全绿；浏览器实测亮色：容器背景 `rgb(238,242,255)→rgb(240,242,245)`、按钮/顶条/Logo/短线渐变均为 `rgb(79,70,229)→rgb(54,207,201)`、h1 `rgb(48,49,51)`；暗色：背景深藏青、渐变起点自动切换为 `rgb(129,140,248)`、h1 浅字。

---

## 附录：CSS 变量速查表

```css
:root {
  /* 主色（靛蓝，暗色下整族重声明为 indigo-400 系，见 §2.1） */
  --color-primary: #4f46e5;
  --color-primary-light: #818cf8;
  --color-primary-lighter: #a5b4fc;
  --color-primary-lightest: #e0e7ff;
  --color-primary-bg: #eef2ff;
  --color-primary-border: #c7d2fe;
  --color-primary-rgb: 79, 70, 229;
  --color-primary-shadow: rgba(79, 70, 229, 0.3);

  /* 功能色 */
  --color-success: #67c23a;
  --color-warning: #e6a23c;
  --color-danger: #f56c6c;
  --color-info: #909399;

  /* 文字色 */
  --text-primary: #303133;
  --text-regular: #606266;
  --text-secondary: #646b77;
  --text-placeholder: #676d78;
  --text-disabled: #a8abb2;

  /* 边框色 */
  --border-base: #ebeef5;
  --border-light: #e4e7ed;
  --border-lighter: #dcdfe6;
  --border-extralight: #f0f2f5;

  /* 背景色 */
  --bg-page: #f0f2f5;
  --bg-card: #ffffff;
  --bg-card-header: #fafbfc;
  --bg-fill: #f5f7fa;

  /* 半透明叠加 / 反白 */
  --color-primary-rgb: 79, 70, 229;
  --color-success-rgb: 103, 194, 58;
  --color-warning-rgb: 230, 162, 60;
  --color-danger-rgb: 245, 108, 108;
  --color-info-rgb: 144, 147, 153;
  --color-accent-rgb: 54, 207, 201;
  --dark-accent-rgb: 79, 195, 247;
  --text-inverse: #ffffff;

  /* 深色代码区 */
  --code-bg: #1e1e1e;
  --code-bg-alt: #1e1e2e;
  --code-panel: #252526;
  --code-border: #2a2a3e;
  --code-border-alt: #2d2d2d;
  --code-text: #d4d4d4;
  --code-scrollbar: #555555;
  --code-scrollbar-hover: #777777;
  --code-key: #9cdcfe;
  --code-string: #ce9178;
  --code-number: #b5cea8;
  --code-boolean: #569cd6;
  --code-attr: #e6c07b;
  --code-muted: #7f848e;
  --code-muted-border: #5c6370;
  --code-danger: #f48771;
  --code-warning: #ffe88a;

  /* 圆角 */
  --radius-card: 10px;
  --radius-table: 8px;
  --radius-button: 6px;
  --radius-tag: 6px;
  --radius-dialog: 14px;
  --radius-picker: 10px;
  --radius-input: 8px;
  --radius-checkbox: 4px;
  --radius-small: 4px;

  /* 间距 */
  --spacing-card-gap: 16px;
  --spacing-card-body: 20px;
  --spacing-form-item: 20px;

  /* 阴影 */
  --shadow-card: 0 1px 4px rgba(0, 0, 0, 0.06);
  --shadow-dialog: 0 20px 60px rgba(0, 0, 0, 0.15);
  --shadow-popper: 0 4px 12px rgba(0, 0, 0, 0.15);
  --shadow-message: 0 8px 24px rgba(0, 0, 0, 0.12);
  --shadow-dropdown: 0 6px 24px rgba(0, 0, 0, 0.12);
  --shadow-picker: 0 8px 28px rgba(0, 0, 0, 0.12);

  /* 过渡 */
  --transition-fast: 0.2s ease;
  --transition-medium: 0.25s ease;
  --transition-slow: 0.3s ease;
  --transition-card-enter: 0.4s ease-out;
}
```
