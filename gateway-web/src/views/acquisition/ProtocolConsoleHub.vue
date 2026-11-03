<template>
  <!-- keep-alive：抓包缓冲、单步队列、回放进度都是会话现场，切页签不应丢失 -->
  <HubShell :tabs="tabs" :group-labels="groupLabels" fallback-tab="register-scan" keep-alive>
    <!-- 统一调试台的目标条：先选总线目标，再用左侧分组页签选动作 -->
    <template #toolbar>
      <ConsoleTargetBar />
    </template>
  </HubShell>
</template>

<script setup lang="ts">
/** 协议调试台：把原「协议扫描 / 协议仿真 / 报文调试」三个 Hub 合并为一个页。
 *
 * 交互主线是「选目标 → 选动作 → 看结果」：
 *  - 顶部工具栏 ConsoleTargetBar 选总线目标（跨页共享，任一页改选其余页跟随）；
 *  - 左侧按分组列出全部动作页签（扫描与探测 / 报文查看与下发 / 模拟与扩展）；
 *  - 内容区渲染当前动作。
 *
 * 页签 name 保持与旧 Hub 完全一致，旧带 ?tab= 的链接经路由重定向后仍能落到同一页，
 * 搜索索引的深链接也无需改写 name。
 *
 * 从站模拟不再做 DEV 门控：后端正转为真实可用，生产构建同样登记该页签。
 */
import { defineAsyncComponent } from 'vue'
import AsyncTabLoading from '@/components/AsyncTabLoading.vue'
import HubShell from '@/components/HubShell.vue'
import type { HubTab } from '@/components/HubShell.vue'
import ConsoleTargetBar from './parts/ConsoleTargetBar.vue'

const ModbusAddressMap = defineAsyncComponent({ loader: () => import('./scan/ModbusAddressMap.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const RegisterScan = defineAsyncComponent({ loader: () => import('./scan/RegisterScan.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const SlaveScan = defineAsyncComponent({ loader: () => import('./scan/SlaveScan.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const AutoDetect = defineAsyncComponent({ loader: () => import('./scan/AutoDetect.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const PacketCapture = defineAsyncComponent({ loader: () => import('./packet/PacketCapture.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const PacketEditor = defineAsyncComponent({ loader: () => import('./packet/PacketEditor.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const PacketReplay = defineAsyncComponent({ loader: () => import('./packet/PacketReplay.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const StepDebug = defineAsyncComponent({ loader: () => import('./packet/StepDebug.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const SlaveSimulator = defineAsyncComponent({ loader: () => import('./simulate/SlaveSimulator.vue'), loadingComponent: AsyncTabLoading, delay: 200 })
const ProtocolConvert = defineAsyncComponent({ loader: () => import('./simulate/ProtocolConvert.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const DebugWizard = defineAsyncComponent({ loader: () => import('./console/DebugWizard.vue'), loadingComponent: AsyncTabLoading, delay: 200 })

const groupLabels = {
  guide: '调试向导',
  scan: '扫描与探测',
  packet: '报文查看与下发',
  simulate: '模拟与扩展',
}

const tabs: HubTab[] = [
  // 向导放在最前：新人第一次进调试台，先沿着「症状 → 检查项」走，再进具体动作页
  { name: 'debug-wizard', label: '现场调试向导', group: 'guide', component: DebugWizard, subtitle: '按协议与症状按顺序执行检查项，可自动检查总线/健康/设备' },
  { name: 'modbus-map', label: 'Modbus 地址映射', group: 'scan', component: ModbusAddressMap, subtitle: '按总线、从站与寄存器类型看地址占用，点格子开占用详情' },
  // 菜单落地优先给扫描能力，而不是 Modbus 专用的地址映射视图
  { name: 'register-scan', label: '寄存器扫描', group: 'scan', component: RegisterScan, subtitle: '按总线、从站与地址区间扫描寄存器，看响应分布并生成点位建议', pin: true },
  // R2：从站扫描负责「找未知地址」，参数探测负责「确认已知地址的参数」，两者入口文案须互相指路
  { name: 'slave-scan', label: '从站扫描', group: 'scan', component: SlaveScan, subtitle: '按总线与地址范围扫描 1–247 从站，用于发现未知从站号；已知地址要确认通信参数请用「参数探测」' },
  { name: 'auto-detect', label: '参数探测', group: 'scan', component: AutoDetect, subtitle: '对已知地址的设备逐项确认通信参数（波特率/校验位等），成功项可复制或跳转应用；不知道从站号先用「从站扫描」' },

  // 先「看」总线上的报文，再「造/发」报文做验证
  { name: 'packet-capture', label: '报文捕获', group: 'packet', component: PacketCapture, subtitle: '观察现场原始帧；抓到的报文可送入编辑与回放' },
  { name: 'packet-editor', label: '报文编辑', group: 'packet', component: PacketEditor, subtitle: '选择总线与超时，填写 HEX 后发送并查看响应；下方附离线帧解析，可粘贴原始帧逐字段核对' },
  { name: 'packet-replay', label: '报文回放', group: 'packet', component: PacketReplay, subtitle: '从回放会话列表选文件，在目标总线逐帧下发给出一致、不一致与失败统计' },
  { name: 'step-debug', label: '单步调试', group: 'packet', component: StepDebug, subtitle: '编辑并逐步下发自定义报文序列，确认真实响应，序列仅存本机浏览器' },

  // 模拟端解决「没有真实设备怎么调」，扩展能力解决「协议不匹配 / 需自定义处理」
  { name: 'slave-simulator', label: '从站模拟', group: 'simulate', component: SlaveSimulator, subtitle: '在网关本机监听 TCP 端口，作为 Modbus TCP 从站供外部主站读写验证' },
  { name: 'protocol-convert', label: '协议转换', group: 'simulate', component: ProtocolConvert, subtitle: '常用值变换模板：原值、线性变换与枚举，免脚本编码' },
]
</script>
