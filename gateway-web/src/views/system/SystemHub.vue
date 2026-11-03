<template>
  <!-- B5 归组口径：按「运维 / 日志 / 安全 / 高级」四类语义重排。首屏直出「运维」7 项，
       「安全」（API 密钥 / 证书签发 / 配置审计）与「高级」（限流规则）默认收起，
       避免低频入口把高频项推出首屏；用户手动展开后的状态仍按路由持久化。
       其中蜂窝网络 / GPIO 会在探测到平台无对应能力时整页隐藏（W1-3）。 -->
  <HubShell
    :tabs="tabs"
    :group-labels="groupLabels"
    :default-collapsed-groups="['security', 'advanced']"
    fallback-tab="overview"
  />
</template>

<script setup lang="ts">
/** 系统 Hub：按「运维 / 日志 / 安全 / 高级」四类语义归组。
 *  - 运维（ops）：网关信息、计划任务、看门狗、Webhook 测试、固件升级、蜂窝网络、GPIO 控制；
 *  - 日志（logs）：实时日志、日志归档；
 *  - 安全（security）：API 密钥、证书签发、配置审计；
 *  - 高级（advanced）：限流规则（默认收起，仅公网/大流量场景按需展开）。
 *  R4：限流规则从「API 密钥」页内卡片收进「高级」分组——现场网关多为局域网直连，
 *  限流属低频配置，不该占一级安全入口；保留全部能力，仅调整入口层级。
 *  R5：原「模拟消费者」页签已迁至「运维监控 › 队列排障」，系统域不再保留该入口。
 *  固件升级 / 证书签发原为「固件与证书」独立 Hub，已并入本 Hub 归并同域功能。
 *  原「运行日志」Hub（实时日志 / 日志归档 / 配置审计）已并入本 Hub：三者同属系统域，
 *  与网关信息一起构成「出了事从哪看」的同一条动线。
 *  「帮助」页已下线：页面内容为硬编码常量，文档迁至文档站（F-15）。
 *  「用户与角色」页已下线（W1）：本网关是单 token 模型，后端 role_id / lastLoginAt 不参与
 *  任何鉴权判定，页面上建的账号登录时不会被校验。A8 起后端 `/api/users` 路由组也已删除，
 *  避免「登记了账号却不生效」的误导。
 *
 * 权限口径：路由不再整体限管理员（否则非管理员会连日志页都进不来），
 * 改由页签级 requireAdmin 承接——运维项与安全项仅管理员可见，
 * 日志项对所有角色开放（所有角色均有 logs.read）。
 */
import { defineAsyncComponent, computed, onMounted, ref } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'
import { getIoStatus } from '@/api/devices'
import { getCellularStatus } from '@/api/network'

const SystemOverview = defineAsyncComponent({ loader: () => import('./overview/SystemOverview.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ApiKeys = defineAsyncComponent({ loader: () => import('./access/ApiKeys.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ScheduledTasks = defineAsyncComponent({ loader: () => import('./maintenance/ScheduledTasks.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const Watchdog = defineAsyncComponent({ loader: () => import('./maintenance/Watchdog.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const WebhookTester = defineAsyncComponent({ loader: () => import('./maintenance/WebhookTester.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const CellularStatus = defineAsyncComponent({ loader: () => import('./maintenance/CellularStatus.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const GpioControl = defineAsyncComponent({ loader: () => import('./maintenance/GpioControl.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ApiRateLimit = defineAsyncComponent({ loader: () => import('./access/ApiRateLimit.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const Ota = defineAsyncComponent({ loader: () => import('./certs/Ota.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const Certificates = defineAsyncComponent({ loader: () => import('./certs/Certificates.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const Logs = defineAsyncComponent({ loader: () => import('./logs/Logs.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const LogArchive = defineAsyncComponent({ loader: () => import('./logs/LogArchive.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ConfigAuditLog = defineAsyncComponent({ loader: () => import('./logs/ConfigAuditLog.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const groupLabels = { ops: '运维', logs: '日志', security: '安全', advanced: '高级' }

/**
 * 平台能力探测（W1）：GPIO 与蜂窝网络是硬件相关页，当前平台不具备时整页隐藏，
 * 避免现场用户点进去只看到「不支持」的空壳。
 * 默认按「支持」展示——探测失败（离线 / 老版本后端无该字段）时不能误隐藏真实可用的页，
 * 只有后端明确返回「不支持 / 未启用」才收敛。
 */
const gpioSupported = ref(true)
const cellularEnabled = ref(true)

const allTabs: HubTab[] = [
  { name: 'overview', label: '网关信息', group: 'ops', requireAdmin: true, component: SystemOverview, subtitle: '版本与资源占用、组件健康、NTP 同步、Webhook、登录会话与诊断报告' },
  { name: 'scheduled-tasks', label: '计划任务', group: 'ops', requireAdmin: true, component: ScheduledTasks, subtitle: '按 cron 定时读取点位、备份配置或同步 NTP，并记录执行结果' },
  { name: 'watchdog', label: '看门狗', group: 'ops', requireAdmin: true, component: Watchdog, subtitle: '服务状态与手动重启、自愈执行器参数、自愈事件时间线' },
  { name: 'webhook-tester', label: 'Webhook 测试', group: 'ops', requireAdmin: true, component: WebhookTester, subtitle: '手工构造请求验证回调地址，保留响应与请求历史' },
  { name: 'ota', label: '固件升级', group: 'ops', requireAdmin: true, component: Ota, subtitle: '上传固件包并执行在线升级或回滚' },
  { name: 'cellular-status', label: '蜂窝网络', group: 'ops', requireAdmin: true, component: CellularStatus, subtitle: '4G/5G 模块信号、流量与拨号状态' },
  { name: 'gpio-control', label: 'GPIO 控制', group: 'ops', requireAdmin: true, component: GpioControl, subtitle: '4 路数字输入 / 4 路数字输出（仅 Linux）' },
  // 日志对所有角色开放（logs.read 全角色都有），不设 requireAdmin
  { name: 'logs', label: '实时日志', group: 'logs', component: Logs, subtitle: '实时日志流、级别与关键词过滤、诊断命令执行' },
  { name: 'log-archive', label: '日志归档', group: 'logs', component: LogArchive, subtitle: '按日期列出网关日志文件，查看日志尾部并导出下载' },
  { name: 'api-keys', label: 'API 密钥', group: 'security', requireAdmin: true, component: ApiKeys, subtitle: '签发与删除 API 密钥，查看权限范围、调用次数与过期时间' },
  { name: 'certificates', label: '证书签发', group: 'security', requireAdmin: true, component: Certificates, subtitle: '当前生效证书状态、上传证书或重新生成自签名，查看类型、有效期与绑定服务' },
  { name: 'audit-log', label: '配置审计', group: 'security', requireAdmin: true, component: ConfigAuditLog, subtitle: '记录配置变更的时间、来源、操作对象与详情' },
  // R4：限流规则收进「高级」分组（默认收起），能力保留但不再占安全组一级入口
  { name: 'api-rate-limit', label: '限流规则', group: 'advanced', requireAdmin: true, component: ApiRateLimit, subtitle: '按路径与方法限制每分钟/每小时请求及突发余量，附命中日志；仅公网或大流量访问时按需启用' },
]

const tabs = computed<HubTab[]>(() =>
  allTabs.filter((t) => {
    if (t.name === 'gpio-control') return gpioSupported.value
    if (t.name === 'cellular-status') return cellularEnabled.value
    return true
  }),
)

onMounted(async () => {
  const [io, cellular] = await Promise.allSettled([getIoStatus(), getCellularStatus()])
  if (io.status === 'fulfilled' && io.value.platform_supported === false) gpioSupported.value = false
  if (cellular.status === 'fulfilled' && cellular.value.enabled === false) cellularEnabled.value = false
})
</script>
