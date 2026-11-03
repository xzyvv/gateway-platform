<template>
  <HubShell :tabs="tabs" :group-labels="groupLabels" fallback-tab="bus-load" />
</template>

<script setup lang="ts">
/**
 * 运维监控 Hub：运行观察（总线负载 / 采集诊断 / 采集统计 / 运行指标 / 队列排障）、时序分析（采集甘特图）、
 * 连通诊断（连接测试）、资源（磁盘健康）、自诊断与报告（自诊断 / 健康报告）、现场交付（交付验收）。
 *
 * 原「性能监控 / 链路诊断 / 健康巡检」三个 Hub 已合并：三者的排查动线是
 * 「看负载 → 测连通 → 查自诊断 → 出健康报告」，分散在三个入口只会让现场反复跳页。
 *
 * 计划任务只在「系统 → 计划任务」保留唯一入口（同一组件挂两处会让用户以为有两套任务）。
 * 页签若用横向标签栏必然换行溢出，且 el-tabs 的 lazy 是「首次激活后永久常驻」，
 * 点一遍就是多套轮询/图表/定时器常驻。分组后左侧竖排不换行，切走即卸载。
 *
 * 「性能压测」页已下线：压测依赖外挂施压端，网关侧只能做被动观测，
 * 仓库内已有 gateway-core/scripts/perf_baseline.py + nightly-load CI 承担该职责。
 * 「波形观测」页已下线（W1）：它对同一批 /api/devices/point-quality 历史点在前端二次绘图，
 * 与「运行总览 › 实时数据」的趋势图同源；实时数据页还多提供值/质量/收藏筛选。
 * 原「链路诊断」Hub 的总线抓包 / 时延分析 / 异常码速查已并入其它页或下线（见历史提交）。
 * 原「健康巡检」Hub 的「实时订阅」「MTBF」两页已下线：前者只暴露常量性指标，
 * 后者统计口径是调度器内存态、重启即清零，可靠性判断以「健康报告」的分级处置为准。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'

const BusLoad = defineAsyncComponent({ loader: () => import('./bus/BusLoad.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const TaskDiagnosis = defineAsyncComponent({ loader: () => import('./performance/TaskDiagnosis.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const StatsReport = defineAsyncComponent({ loader: () => import('./performance/StatsReport.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const MetricsDashboard = defineAsyncComponent({ loader: () => import('./performance/MetricsDashboard.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const QueueTriage = defineAsyncComponent({ loader: () => import('./queue/QueueTriage.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const GanttChart = defineAsyncComponent({ loader: () => import('./performance/GanttChart.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ConnectionTest = defineAsyncComponent({ loader: () => import('./connection/ConnectionTest.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DiskHealthChart = defineAsyncComponent({ loader: () => import('./health/DiskHealthChart.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const GatewayDiag = defineAsyncComponent({ loader: () => import('./health/GatewayDiag.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const HealthReport = defineAsyncComponent({ loader: () => import('./health/HealthReport.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const DeliveryCheck = defineAsyncComponent({ loader: () => import('./delivery/DeliveryCheck.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

// 分组按「看现状 → 看时序 → 测连通 → 查资源 → 综合判断」的排查动线排列
const groupLabels = {
  monitor: '运行观察',
  analyze: '时序分析',
  connection: '连通诊断',
  resource: '资源',
  reliability: '自诊断与报告',
  delivery: '现场交付',
}

const tabs: HubTab[] = [
  { name: 'bus-load', label: '总线负载', group: 'monitor', component: BusLoad, subtitle: '各总线使用率、收发速率、错误率与平均延迟，附收发负载趋势图' },
  { name: 'task-diagnosis', label: '采集诊断', group: 'monitor', component: TaskDiagnosis, subtitle: '选择点位运行诊断，按诊断项查看真实诊断结果、状态与处理建议' },
  { name: 'stats-report', label: '采集统计', group: 'monitor', component: StatsReport, subtitle: '日报周报月报的总采集次数、成功率与延迟，含设备与总线汇总和错误分布' },
  { name: 'metrics-dashboard', label: '运行指标', group: 'monitor', component: MetricsDashboard, subtitle: '上行总数、队列长度、在线设备与错误总数，及按指标分组的采样明细' },
  // R5：模拟消费者从系统域迁入此处，与队列长度指标同一排查动线（先看积压，再用兜底消费者消歧）
  { name: 'queue-triage', label: '队列排障', group: 'monitor', component: QueueTriage, subtitle: '上行队列积压排查：查看兜底消费者运行态与消费指标，无 MQTT 通道时可启动模拟消费者排空队列' },
  { name: 'gantt-chart', label: '采集甘特图', group: 'analyze', component: GanttChart, subtitle: '10秒窗口内各设备采集任务时序，标出时间冲突与总线负载建议' },
  { name: 'connection-test', label: '连接测试', group: 'connection', component: ConnectionTest, subtitle: '逐个测试设备采集链路与上行通道，汇总通过数、失败数与平均耗时；原始帧请用「采集 › 报文调试 › 报文捕获」，失败原因可在页内「异常码速查」对照' },
  { name: 'disk-health', label: '磁盘健康', group: 'resource', component: DiskHealthChart, subtitle: '剩余空间、采样点数与危险阈值趋势' },
  { name: 'gateway-diag', label: '自诊断', group: 'reliability', component: GatewayDiag, subtitle: 'CPU 温度与使用率、内存磁盘占用，网络、串口与系统信息' },
  // P1-3：/api/health/report 后端早已实现，接成页签让「队列背压/总线全离线」在 UI 上直接可见
  { name: 'health-report', label: '健康报告', group: 'reliability', component: HealthReport, subtitle: '综合状态、组件状态、逐总线明细与分级处置问题' },
  // P2 交付验收包：默认配置模板 + 一键自检 + 交付检查单
  { name: 'delivery-check', label: '交付验收', group: 'delivery', component: DeliveryCheck, subtitle: '套用默认配置模板、一键自检六项并导出交付检查单，现场不看文档即可完成验收' },
]
</script>
