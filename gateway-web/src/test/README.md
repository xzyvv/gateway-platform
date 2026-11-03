# 测试约定

> 路径：`src/test/setup.ts`（全局初始化）
> 当前：35 个 .spec.ts 文件，266 个用例；另有 `e2e/` 下 6 条 Playwright 冒烟（见文末）

## 测试分布

| 类别 | 代表文件 | 说明 |
|------|----------|------|
| composables | useDevices / useGatewayDiscovery / usePoints / useLayoutClock / useGatewaySwitcher | 单元测试 |
| stores | websocket 等 | store 逻辑 |
| utils | deviceCopy / exportCsv / importPoints / prometheus / secureStorage | 工具函数 |
| router | routes/redirects / routerError | 重定向目标存在性与路由级错误兜底 |
| components | PageErrorBoundary | 通用组件 |
| views | Dashboard / Devices / Points / Config / DataFlowTrace / Simulator / Sidebar / Topbar / LoadFailedRoute / connection 表单组等 | 页面级与子组件 |
| mock | mocks | mock 数据形状冒烟 |

## 测试约定

### 1. 位置：co-locate

`.spec.ts` 文件与被测源文件同目录，禁止放在 `__tests__/` 子目录。

```ts
src/views/devices/
├── Devices.vue
└── Devices.spec.ts    ← 同目录
```

### 2. 显式导入 vitest API

即使 `globals: true`，项目所有 spec 文件**显式 import** 测试 API：

```ts
import { describe, it, expect, beforeEach, vi } from 'vitest'
```

不要依赖全局注入，便于类型检查。

### 3. 全局组件注册

`setup.ts` 中把常用展示组件注册到 `@vue/test-utils` 的 `config.global.components`：

- EmptyState
- SectionHeader

测试组件时不用 import 这些组件。

### 4. Mock 子组件

重型子组件（如 echarts）通过 `vi.mock` 跳过：

```ts
vi.mock('./TrendChart.vue', () => ({ default: { name: 'TrendChart', render: () => null } }))
```

### 5. 异步等待

用 `@vue/test-utils` 的 `flushPromises` + `nextTick` 等待组件更新：

```ts
async function settle() {
  await flushPromises()
  await nextTick()
}
```

## 添加新 spec 文件模板

```ts
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount } from '@vue/test-utils'
import TargetComp from './TargetComp.vue'

describe('TargetComp.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders title and template/preview sections', async () => {
    const wrapper = mount(TargetComp, { props: { /* ... */ } })
    await flushPromises()
    expect(wrapper.text()).toContain('预期文字')
  })
})
```

## 覆盖率目标

`vitest.config.ts` 按一级目录设**防回落地板**（补测试只许上调、不许下调）：

- 顶层兜底：lines 30% / functions 25% / branches 20% / statements 30%
- 业务重点目录更高，如 views（lines 59%）、utils（50%）、composables（40%）
- 具体数值以 `vitest.config.ts` 的 `coverage.thresholds` 为准

新增模块若涉及业务逻辑必须配套 spec，否则低于所在目录阈值会被 CI 拦截。

## E2E（Playwright）

浏览器级冒烟独立放在 `e2e/`，不参与 vitest 与覆盖率统计：`npm run e2e` 一条命令自动编排空配置 gatewayd 与 vite dev server，跑 6 条只读核心链路（栈健康 / 登录 / 路由巡游 / 暗色 / 404 / 登出）。用例约定与 CI 门禁状态见 [`docs/architecture/frontend.md`](../../../docs/architecture/frontend.md) 3.5 节。

## 已知排除

`vitest.config.ts` 中已 exclude：

- `src/api/generated/**`（自动生成的类型 stub）
- `src/**/*.css` / `*.vue` 的样式块
- `src/**/*.d.ts`
- `src/test/setup.ts`（自身）
- `*.config.ts` / `*.config.js`（构建配置）