<template>
  <el-header class="header">
    <div class="header-left">
      <el-button
        text
        class="collapse-btn"
        :aria-label="navToggleLabel"
        :title="navToggleLabel"
        @click="$emit('toggle-collapse')"
      >
        <el-icon><MenuIcon v-if="mobile" /><Fold v-else-if="!collapsed" /><Expand v-else /></el-icon>
      </el-button>
      <el-breadcrumb separator="/" class="header-breadcrumb">
        <!-- 面包屑只表达真实层级：分组 / 当前页 / 页签。
             三层均可点击：分组→该组首个菜单、页面→当前路由（清掉 query 回到默认 tab）、
             页签→保持当前 tab。不再挂指向 /overview 的「首页」。
             页签与当前页同名时不再重复一层（如「采集 / 点位读写 / 点位读写」）。 -->
        <el-breadcrumb-item v-if="group" :to="groupLink">{{ group }}</el-breadcrumb-item>
        <el-breadcrumb-item :to="pageLink">{{ title }}</el-breadcrumb-item>
        <el-breadcrumb-item v-if="hubTab && hubTab !== title" :to="hubTabLink">{{ hubTab }}</el-breadcrumb-item>
      </el-breadcrumb>
    </div>
    <div class="header-right">
      <!-- 网关切换（A3 修复：禁用 + loading） -->
      <el-dropdown
        v-if="isTauriEnv"
        trigger="click"
        @command="(c: string) => gatewaySwitcher.onGatewayCmd(c)"
        :disabled="isSwitchingGateway"
      >
        <el-button text class="gateway-switcher" :title="activeGatewayUrl" :loading="isSwitchingGateway">
          <el-icon><Connection /></el-icon>
          <span class="gateway-name">{{ activeGatewayName || '未配置' }}</span>
          <el-icon class="el-icon--right"><ArrowDown /></el-icon>
        </el-button>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item
              v-for="g in gateways"
              :key="g.name"
              :command="'switch:' + g.name"
              :disabled="g.name === activeGatewayName || isSwitchingGateway"
            >
              <el-icon><Monitor /></el-icon> {{ g.name }}
              <span v-if="g.name === activeGatewayName" class="gw-active-tag">当前</span>
            </el-dropdown-item>
            <el-dropdown-item command="add" divided :disabled="isSwitchingGateway">
              <el-icon><Plus /></el-icon> 新增网关...
            </el-dropdown-item>
            <el-dropdown-item command="scan" :disabled="isSwitchingGateway || isScanning">
              <el-icon><Search /></el-icon> {{ isScanning ? '扫描中...' : '扫描局域网...' }}
            </el-dropdown-item>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
      <div class="topbar-clock" :title="now">
        <el-icon><Clock /></el-icon>
        <span class="topbar-clock-time">{{ now }}</span>
      </div>
      <el-button text class="icon-btn density-btn" @click="toggleDensity"
        :aria-label="density === 'compact' ? '切换表格密度为标准' : '切换表格密度为紧凑'"
        :title="density === 'compact' ? '表格密度：紧凑（点击切换为标准）' : '表格密度：标准（点击切换为紧凑）'">
        <el-icon><Grid /></el-icon>
      </el-button>
      <el-button text class="icon-btn" @click="clock.toggleTheme" :aria-label="isDark ? '切换亮色主题' : '切换暗色主题'" :title="isDark ? '切换亮色' : '切换暗色'">
        <el-icon><Sunny v-if="isDark" /><Moon v-else /></el-icon>
      </el-button>
      <!-- 通知中心：写操作失败只留痕不弹窗，这里是唯一的事后可查入口 -->
      <NotificationCenter />
      <!-- 健康状态徽章：正常时显示「服务在线」，有异常时优先显示「N 项异常」（黄色），
           断连时显示「失联」并可点开建议操作下拉（重试 / 切备用 / 放弃在飞 / 详情） -->
      <el-dropdown
        trigger="click"
        :disabled="!(gatewayUnreachable || anomalyCount > 0)"
        class="gw-triage"
        @command="onTriageCmd"
      >
        <span class="badge-with-actions" :title="healthBadgeTitle">
          <div class="health-badge" :class="healthOkClass">
            <span class="health-dot"></span>
            <span>{{ healthOkText }}</span>
          </div>
          <el-icon v-if="gatewayUnreachable || anomalyCount > 0" class="triage-caret"><ArrowDown /></el-icon>
        </span>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item v-if="gatewayUnreachable" command="retry">立即重连（1 秒后再次检测）</el-dropdown-item>
            <el-dropdown-item v-if="gatewayUnreachable && isTauriEnv && gateways.length > 1" command="switch">切备用网关</el-dropdown-item>
            <el-dropdown-item v-if="hasInflight" command="abort">放弃在飞请求（{{ inflightCount }} 个）</el-dropdown-item>
            <el-dropdown-item command="details" divided>查看异常详情...</el-dropdown-item>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
      <!-- 安全风险徽章：默认凭据 / 匿名只读 / 开放 CORS 等风险常驻提醒。
           以前是每页顶部的黄色横幅，首屏占高且每次会话都要关一次；
           现在只在顶栏留一枚徽章，点开详情抽屉看具体条目与处理入口。 -->
      <el-tooltip
        v-if="securityItems.length"
        placement="bottom"
        :content="`存在 ${securityItems.length} 项安全风险，点击查看详情`"
      >
        <button
          type="button"
          class="security-badge"
          :aria-label="`安全风险 ${securityItems.length} 项，查看详情`"
          @click="$emit('open-security')"
        >
          <el-icon><WarningFilled /></el-icon>
          <span>安全风险 {{ securityItems.length }}</span>
        </button>
      </el-tooltip>
      <!-- WS 连接状态徽章：仅重连中/断开时显示（已连接时完全隐藏，避免顶栏冗余） -->
      <WsStatusBadge />
      <el-dropdown
        v-if="isDev"
        trigger="click"
        @command="onTierCmd"
        class="tier-switcher"
      >
        <el-button text :title="`当前档位：${tierDisplayLabel}（${TIER_DESCRIPTION[tier.current]}）`" class="tier-btn">
          <el-icon><Trophy /></el-icon>
          <span class="tier-label">{{ tierDisplayLabel }}</span>
          <el-icon class="el-icon--right"><ArrowDown /></el-icon>
        </el-button>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item
              v-for="t in tierOptions"
              :key="t"
              :command="'tier:' + t"
              :disabled="t === tier.current"
            >
              <el-icon><component :is="t === 'ultra' ? StarFilled : (t === 'pro' ? Medal : GoldMedal)" /></el-icon>
              {{ TIER_LABEL[t] }} <span class="tier-hint">{{ TIER_DESCRIPTION[t] }}</span>
            </el-dropdown-item>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
      <!-- License 状态徽章（生产模式，§11.2：ok 绿 / expiring 黄 / 降级红）；点击打开授权管理。
           用「钥匙」图标而非绿点：左侧健康徽章已用圆点表示在线状态，两个绿点并排会让人以为重复。 -->
      <div
        v-else
        class="health-badge license-badge"
        :class="licenseBadgeClass"
        :title="licenseBadgeTitle"
        role="button"
        tabindex="0"
        @click="$emit('open-license')"
        @keydown.enter.prevent="$emit('open-license')"
        @keydown.space.prevent="$emit('open-license')"
      >
        <el-icon class="license-icon"><Key /></el-icon>
        <span>{{ licenseBadgeText }}</span>
        <!-- v2.0：配额用量（上限 + 已用），让用户不用点开授权对话框就知道剩余空间 -->
        <span v-if="licenseQuotaText" class="license-quota">{{ licenseQuotaText }}</span>
      </div>

      <el-dropdown trigger="click" @command="onUserCmd">
        <el-button text class="logout-btn">
          <el-icon><User /></el-icon>
          <span class="logout-text">{{ clock.username || '未登录' }}</span>
          <el-icon class="el-icon--right"><ArrowDown /></el-icon>
        </el-button>
        <template #dropdown>
          <el-dropdown-menu>
            <el-dropdown-item command="change-password">
              <el-icon><Lock /></el-icon> 修改密码
            </el-dropdown-item>
            <el-dropdown-item command="open-license">
              <el-icon><Key /></el-icon> 授权管理
            </el-dropdown-item>
            <el-dropdown-item command="request-monitor">
              <el-icon><Monitor /></el-icon> 请求监视器
            </el-dropdown-item>
            <!-- 现场支持闭环：把版本/健康/设备探测/近期错误打包发给厂商 -->
            <el-dropdown-item command="diagnostic">
              <el-icon><Document /></el-icon> 导出诊断报告
            </el-dropdown-item>
            <el-dropdown-item command="logout" divided>
              <el-icon><SwitchButton /></el-icon> 退出登录
            </el-dropdown-item>
          </el-dropdown-menu>
        </template>
      </el-dropdown>
    </div>
  </el-header>
</template>

<script setup lang="ts">
/**
 * Layout 顶栏
 *
 * 负责：折叠按钮 + 面包屑 + 网关切换 dropdown + 时钟 + 主题切换 + 表格密度
 *       + 健康徽章 + DEV tier dropdown + License 徽章（点击打开授权管理）
 *       + 用户菜单（修改密码/授权管理/请求监视器/诊断报告/退出）
 *
 * 通过 props 接受外部状态/方法（clock / gatewaySwitcher），
 * 业务行为通过 emits 上交父组件处理（折叠、退出、tier 切换 等副作用逻辑留在 Layout）。
 */
import { computed, onUnmounted } from 'vue'
import { useRoute } from 'vue-router'
import { ElMessage } from 'element-plus'
import {
  Search, Fold, Expand, Connection, ArrowDown, Clock, Sunny, Moon,
  User, Lock, SwitchButton, Plus, Trophy, GoldMedal, Medal, StarFilled, Key, Grid, Monitor, Document,
  WarningFilled,
  Menu as MenuIcon,
} from '@element-plus/icons-vue'
import { useTierStore, TIER_LABEL, TIER_DESCRIPTION, type Tier } from '../../stores/tier'
import { useTableDensity } from '@/composables/core/useTablePrefs'
import NotificationCenter from '@/components/NotificationCenter.vue'
import WsStatusBadge from '@/components/WsStatusBadge.vue'
import type { useLayoutClock } from '@/composables/layout/useLayoutClock'
import type { useGatewaySwitcher } from '@/composables/layout/useGatewaySwitcher'
import { getInflightCount, abortAllInflight } from '@/api'
import { formatDate } from '@/utils/datetime'
import { findGroupFirstPath } from './menu'

type LayoutClockApi = ReturnType<typeof useLayoutClock>
type GatewaySwitcherApi = ReturnType<typeof useGatewaySwitcher>

const props = defineProps<{
  /** 是否折叠（受控） */
  collapsed: boolean
  /** 时钟/健康/主题 composable 实例 */
  clock: LayoutClockApi
  /** 网关切换 composable 实例 */
  gatewaySwitcher: GatewaySwitcherApi
  /** 当前路由 meta.title */
  title?: string
  /** 当前路由所在分组（来自 Sidebar 的 currentGroup） */
  group?: string
  /** Hub 页当前子页签（来自 HubShell），用于补全第三层面包屑 */
  hubTab?: string
  /** 当前网关的安全风险条目（默认凭据 / 开放 CORS 等）；非空时显示风险徽章 */
  securityWarnings?: string[]
  /** 窄屏抽屉模式：折叠按钮退化为「打开导航抽屉」的汉堡按钮 */
  mobile?: boolean
}>()

const emit = defineEmits<{
  (e: 'toggle-collapse'): void
  (e: 'open-license'): void
  (e: 'open-request-monitor'): void
  (e: 'open-security'): void
  (e: 'logout'): void
  (e: 'change-password'): void
  (e: 'tier-change', tier: Tier): void
  (e: 'open-health-detail'): void
  (e: 'diagnostic-report'): void
}>()

const tier = useTierStore()
const route = useRoute()

/**
 * 面包屑三层的跳转目标：
 *   - 分组层 → 该分组首个菜单项（如「数据」→ /data）
 *   - 页面层 → 当前路由 path（清掉 query，Hub 页回到默认 tab）
 *   - 页签层 → 当前路由 + 保留 ?tab=（停在当前子页）
 */
const groupLink = computed(() => (props.group ? findGroupFirstPath(props.group) : ''))
const pageLink = computed(() => route.path)
const hubTabLink = computed(() => ({ path: route.path, query: route.query }))

/** 断连态「重试」后延迟 1s 的补检句柄；卸载/重复点击时需清掉，避免对已销毁的壳再打请求 */
let retryProbeTimer: ReturnType<typeof setTimeout> | null = null
onUnmounted(() => {
  if (retryProbeTimer) {
    clearTimeout(retryProbeTimer)
    retryProbeTimer = null
  }
})

// 表格密度是全站偏好：放在顶栏是为了让「当前是紧凑还是标准」随时可见、可改，
// 否则用户进了某个没表格的页面就找不到这个开关了。
const { density, toggleDensity } = useTableDensity()

// props 上的 composable 实例是普通对象，其 Ref 属性在子组件模板中不会自动解包，
// 这里把模板用到的响应式字段统一取为 computed 别名（方法仍直接调 props 实例上的函数）。
const isTauriEnv = computed(() => props.gatewaySwitcher.isTauriEnv)
const gateways = computed(() => props.gatewaySwitcher.gateways.value)
const activeGatewayName = computed(() => props.gatewaySwitcher.activeGatewayName.value)
const activeGatewayUrl = computed(() => props.gatewaySwitcher.activeGatewayUrl.value)
const isSwitchingGateway = computed(() => props.gatewaySwitcher.isSwitchingGateway.value)
const isScanning = computed(() => props.gatewaySwitcher.isScanning.value)
const now = computed(() => props.clock.now.value)
const isDark = computed(() => props.clock.isDark.value)
/** 导航开关按钮的语义：窄屏下它打开抽屉，而不是折叠 220px 侧栏 */
const navToggleLabel = computed(() => {
  if (props.mobile) return '打开侧边导航'
  return props.collapsed ? '展开侧边导航' : '收起侧边导航'
})
const healthOkClass = computed(() => props.clock.healthOkClass.value)
const healthOkText = computed(() => props.clock.healthOkText.value)
/** 异常条数（组件降级 + 队列积压）：徽章文案与下拉提示共用同一口径 */
const anomalyCount = computed(() => props.clock.anomalyCount.value)
const healthBadgeTitle = computed(() => {
  if (gatewayUnreachable.value) return '网关失联：页面上的数据可能是历史值'
  const parts = [...props.clock.degradedComponents.value]
  // 队列积压只计数、不出现在组件明细里时，补一条说明，避免徽章说「N 项异常」而提示里查无此项
  if (props.clock.anomalyCount.value > parts.length) parts.push('上行队列积压')
  if (parts.length) return `异常明细：${parts.join('、')}`
  if (props.clock.healthOk.value === false) return '健康检查未通过，点开查看详情'
  return '网关连接正常，所有组件运行中'
})

// 网关断连态判断（用于建议操作下拉）
const gatewayUnreachable = computed(() => props.clock.gatewayUnreachable.value)
const hasInflight = computed(() => getInflightCount() > 0)
const inflightCount = computed(() => getInflightCount())

const isDev = import.meta.env.DEV
const tierOptions: Tier[] = ['core', 'standard', 'pro', 'ultra']

/** 可选数组 prop 在模板里直接 .length 会在未传时抛错，统一兜底为空数组 */
const securityItems = computed(() => props.securityWarnings ?? [])

// ===== License 状态徽章（生产模式渲染，§11.2）=====
// 状态色语义：ok=绿（正常档位）、expiring=黄（临期提醒）、其余失效态=红（降级警示）
const LICENSE_DEGRADED_TEXT: Partial<Record<string, string>> = {
  expired: '授权已过期',
  mismatch: '授权不匹配',
  tampered: '授权异常',
  invalid: '授权无效',
}
const licenseBadgeClass = computed(() => {
  const st = tier.licenseStatus
  const s = tier.licenseState
  if (st && LICENSE_DEGRADED_TEXT[st]) return 'invalid'
  if (s && !s.present) return 'expiring'
  if (st === 'expiring') return 'expiring'
  return 'ok'
})
/** 没有有效授权文件时不能再显示「核心版」冒充已授权，明确标为未授权 */
const tierDisplayLabel = computed(() => {
  const s = tier.licenseState
  if (s && !s.present) return '未授权'
  return TIER_LABEL[tier.current]
})
const licenseBadgeText = computed(() => {
  const st = tier.licenseStatus
  if (st && LICENSE_DEGRADED_TEXT[st]) return LICENSE_DEGRADED_TEXT[st]
  return tierDisplayLabel.value
})
/**
 * v2.0：徽章旁的点位用量（如 "5000 点（已用 1820）"，见设计文档 §5.1）。
 *
 * 仅在配额**有上限**且用量已知时显示——不限额时展示"已用 N 点"没有意义
 * （用户看到"不限"已经知道没有上限约束），反而挤占顶栏空间。
 * 旧版本网关无 usage 字段 → 不显示，不伪造数字。
 */
const licenseQuotaText = computed(() => {
  const max = tier.licenseState?.quota?.max_points
  const used = tier.licenseUsage?.points
  if (typeof max !== 'number' || typeof used !== 'number') return ''
  return `${max} 点（已用 ${used}）`
})
const licenseBadgeTitle = computed(() => {
  const s = tier.licenseState
  if (!s) return '授权状态未获取'
  const parts: string[] = []
  if (!s.present) parts.push('未检测到授权文件，当前按最低档（核心版）配额运行')
  parts.push(`档位: ${TIER_LABEL[s.tier]}`)
  if (s.customer) parts.push(`客户: ${s.customer}`)
  if (s.license_id) parts.push(`编号: ${s.license_id}`)
  // ts-rs 导出 bigint，axios 实际解析为 number，Number() 双重兜底
  if (s.expires_at != null) {
    parts.push(`到期: ${formatDate(Number(s.expires_at) * 1000)}`)
  }
  if (s.status === 'expiring') parts.push('授权即将到期，请联系供应商续期')
  // v2.0：配额用量明细（上限 / 已用 / 剩余），让用户一眼看出离上限还有多远
  const usage = tier.licenseUsage
  const avail = tier.licenseAvailable
  if (usage && avail) {
    const q = s.quota
    const fmtLimit = (v: number | null | undefined) => (v == null ? '不限' : String(v))
    const fmtAvail = (v: number | null | undefined) => (v == null ? '不限' : String(v))
    parts.push(
      `点位: ${fmtLimit(q.max_points)}（已用 ${usage.points}，剩余 ${fmtAvail(avail.points)}）`,
    )
    parts.push(
      `协议: ${fmtLimit(q.max_protocols)}（已用 ${usage.protocols}，剩余 ${fmtAvail(avail.protocols)}）`,
    )
    parts.push(
      `保留: ${fmtLimit(q.storage_days)} 天（实际 ${usage.storage_days} 天）`,
    )
    parts.push(
      `通道: ${fmtLimit(q.northbound_channels)}（已用 ${usage.northbound_channels}，剩余 ${fmtAvail(avail.northbound_channels)}）`,
    )
  }
  return parts.join('　')
})

function onUserCmd(cmd: string) {
  if (cmd === 'logout') emit('logout')
  else if (cmd === 'change-password') emit('change-password')
  else if (cmd === 'open-license') emit('open-license')
  else if (cmd === 'request-monitor') emit('open-request-monitor')
  else if (cmd === 'diagnostic') emit('diagnostic-report')
}

function onTierCmd(cmd: string) {
  if (!isDev) return
  if (cmd.startsWith('tier:')) {
    const next = cmd.slice(5) as Tier
    emit('tier-change', next)
  }
}

/** 网关断连态建议操作下拉（retry / switch / abort / details） */
function onTriageCmd(cmd: string) {
  if (cmd === 'retry') {
    // 立即重连 + 延迟 1s 再次检测（给之前的 checkHealth 周期让位）
    props.clock.checkHealth()
    if (retryProbeTimer) clearTimeout(retryProbeTimer)
    retryProbeTimer = setTimeout(() => { retryProbeTimer = null; props.clock.checkHealth() }, 1000)
    ElMessage.info('已触发重连检测')
  } else if (cmd === 'switch') {
    const candidates = gateways.value.filter((g) => g.name !== activeGatewayName.value)
    if (candidates.length) props.gatewaySwitcher.performGatewaySwitch(candidates[0]!.name)
    else ElMessage.warning('未找到备用网关')
  } else if (cmd === 'abort') {
    const n = getInflightCount()
    abortAllInflight('user request from topbar')
    ElMessage.info(`已放弃 ${n} 个在飞请求`)
  } else if (cmd === 'details') {
    emit('open-health-detail')
  } else if (cmd === 'diagnostic') {
    emit('diagnostic-report')
  }
}
</script>

<style src="./Topbar.css" />