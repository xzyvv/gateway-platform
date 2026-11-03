/**
 * 路由定义（Layout 下的全部页面路由）。
 *
 * 命名约定：
 * - 一级路径 = 业务域：overview / devices / points / acquisition / data / uplink / ops / system。
 * - 二级路径 = 视图能力：`/<域>/<能力>`，如 `/devices/topology`、`/system/config`。
 * - Hub 页的 tab 用 `?tab=<name>` 表达，不再为每个 tab 单独登记 redirect 别名。
 * - `name` 与页面组件名保持一致，便于从路由反查文件。
 * - `title` 放在 `meta` 中（Vue Router 只把 meta 合并进 to.meta），
 *   面包屑（Layout.vue）、document.title（main.ts afterEach）与 admin 守卫均从 meta 读取。
 *
 * ── 新增页面登记清单（防漏项）─────────────────────────────
 *  1. 路由：本文件按业务域登记；若页面是 Hub 内 tab，只需在 Hub 页登记 tab，不新增路由
 *  2. 菜单：views/layout/menu/groups/*.ts 对应组加入口
 *  3. 搜索索引：views/layout/menu/navIndex.ts 加 nav- 项（侧栏搜索可达）
 *  4. Hub 内 tab：在所属 Hub 页（如 ProtocolConsoleHub.vue）的 tabs 数组登记
 * ─────────────────────────────────────────────────────────
 */

import type { RouteRecordRaw } from 'vue-router'

export const routes: RouteRecordRaw[] = [
  // ── 概览 ────────────────────────────────────────
  {
    path: '/overview',
    name: 'Overview',
    component: () => import('@/views/overview/Overview.vue'),
    meta: { title: '运行总览', pageHeader: true },
  },
  {
    path: '/overview/realtime',
    name: 'RealtimeMonitor',
    component: () => import('@/views/overview/RealtimeMonitor.vue'),
    meta: { title: '实时数据' },
  },

  // ── 设备 ────────────────────────────────────────
  {
    path: '/devices',
    name: 'DeviceHub',
    component: () => import('@/views/devices/DeviceHub.vue'),
    // hub: 页面标题由 HubShell 的页签 h1 承担，Layout 不再补 sr-only h1
    meta: { title: '设备管理', hub: true },
  },
  {
    // 原「设备运维」Hub 已并入设备 Hub 的「运行评估 / 干预与镜像」分组，
    // 旧地址保留重定向，兼容旧书签与外部链接。
    path: '/devices/ops',
    redirect: '/devices?tab=health-score',
  },
  {
    // 原「快速接入」三步骤向导（总线 → 设备 → 点位 + 连读）与「配置管理 › 部署向导」
    // 是同一件事的两套壳，后者接真后端且可回退，故向导页删除，旧地址直达向导页签。
    path: '/devices/quick-setup',
    name: 'QuickSetupRedirect',
    redirect: '/system/config?tab=wizard',
  },
  {
    path: '/devices/topology',
    name: 'DeviceTopology',
    component: () => import('@/views/devices/DeviceTopology.vue'),
    meta: { title: '设备拓扑' },
  },
  // 批量命令 / 批量导入已接入真实 API，回归「设备管理」Hub；旧地址保留重定向
  {
    path: '/devices/batch-import',
    redirect: '/devices?tab=batch-import',
  },
  {
    path: '/devices/batch-command',
    redirect: '/devices?tab=batch-command',
  },

  // ── 点位 ────────────────────────────────────────
  {
    path: '/points',
    name: 'PointHub',
    component: () => import('@/views/points/PointHub.vue'),
    meta: { title: '点位管理', hub: true },
  },
  {
    // 原「地址换算」Hub 已并入点位 Hub 的「进阶工具」分组，旧地址保留重定向。
    path: '/points/address-tools',
    redirect: '/points?tab=addr-calculator',
  },

  // ── 采集（点位读写 / 协议调试台 / 采集调优）──────────
  {
    path: '/acquisition/tester',
    name: 'TesterHub',
    component: () => import('@/views/acquisition/TesterHub.vue'),
    meta: { title: '点位读写', hub: true },
  },
  // 第三阶段 C1：原「协议扫描 / 协议仿真 / 报文调试」三个 Hub 合并为统一「协议调试台」，
  // 交互主线为「选目标（顶部总线条）→ 选动作（左侧分组页签）→ 看结果」。
  {
    path: '/acquisition/console',
    name: 'ProtocolConsoleHub',
    component: () => import('@/views/acquisition/ProtocolConsoleHub.vue'),
    meta: { title: '协议调试台', hub: true },
  },
  {
    path: '/acquisition/comm-tuning',
    name: 'CommTuningHub',
    component: () => import('@/views/acquisition/CommTuningHub.vue'),
    meta: { title: '采集调优', hub: true },
  },

  // ── 数据 ────────────────────────────────────────
  {
    path: '/data',
    name: 'DataPipelineHub',
    component: () => import('@/views/data/DataPipelineHub.vue'),
    meta: { title: '数据流水', hub: true },
  },
  {
    // 原「数据质量」Hub 已并入数据流水 Hub 的「质量与治理」分组，旧地址保留重定向。
    path: '/data/quality',
    redirect: '/data?tab=data-quality',
  },

  // ── 上行 ────────────────────────────────────────
  {
    path: '/uplink',
    name: 'UplinkHub',
    component: () => import('@/views/uplink/UplinkHub.vue'),
    // 上行转发：涉及北向地址/凭证/转发规则写入，仅管理员
    meta: { title: '上行转发', requireAdmin: true, hub: true },
  },

  // ── 运维 ────────────────────────────────────────
  {
    path: '/ops',
    name: 'OpsHub',
    component: () => import('@/views/ops/OpsHub.vue'),
    meta: { title: '运维监控', hub: true },
  },
  // 原「性能监控 / 链路诊断 / 健康巡检」三个 Hub 已合并为「运维监控」Hub，
  // 旧地址按原落地页签重定向，兼容旧书签与外部链接。
  {
    path: '/ops/perf',
    redirect: '/ops?tab=bus-load',
  },
  {
    path: '/ops/comm',
    redirect: '/ops?tab=connection-test',
  },
  {
    path: '/ops/health',
    redirect: '/ops?tab=gateway-diag',
  },
  {
    // 「收藏监控」独立页已下线（W1）：与「运行总览 › 实时数据」的收藏筛选（?fav=1）完全同源
    // （同一份 WebSocket 实时流 + 同一份本地收藏表），保留跳转只为兼容旧书签与外部链接。
    path: '/ops/batch-monitor',
    name: 'BatchMonitorRedirect',
    redirect: '/overview/realtime?fav=1',
  },

  // ── 系统 ────────────────────────────────────────
  {
    path: '/system',
    name: 'SystemHub',
    component: () => import('@/views/system/SystemHub.vue'),
    // 原「系统设置」与「运行日志」两个 Hub 已合并：日志对所有角色开放（logs.read），
    // 路由不能整体限管理员，管理员专属项由页签级 requireAdmin 承接。
    meta: { title: '系统设置', hub: true },
  },
  {
    // 原「运行日志」Hub 已并入系统 Hub 的「日志」分组，旧地址保留重定向。
    path: '/system/logs',
    redirect: '/system?tab=logs',
  },
  {
    path: '/system/config',
    name: 'ConfigHub',
    component: () => import('@/views/system/config/ConfigHub.vue'),
    // 配置管理：导入/导出/重载网关配置，仅管理员（operator 仅 config.read）
    meta: { title: '配置管理', requireAdmin: true, hub: true },
  },
]
