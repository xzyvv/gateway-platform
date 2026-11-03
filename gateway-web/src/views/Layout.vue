<template>
  <!-- 键盘用户跳过侧栏与顶栏，直接落到主内容区（获得焦点后按 Tab 即可进入页面） -->
  <a class="skip-link" href="#main-content">跳到主内容</a>
  <el-container class="layout">
    <!-- 窄屏抽屉的遮罩：点击任意空白处收起导航，避免抽屉常驻遮挡内容 -->
    <div v-if="isMobileNav && mobileNavOpen" class="sidebar-mask" @click="mobileNavOpen = false" />
    <Sidebar
      v-model:collapsed="isCollapsed"
      :class="{ 'mobile-drawer': isMobileNav, open: isMobileNav && mobileNavOpen }"
    />

    <!-- 必须显式 direction="vertical"：Element Plus 的 ElContainer 只判断直接子节点
         vNode 是否为 ElHeader/ElFooter 来决定加不加 is-vertical，而这里的直接子节点是
         自定义组件 <Topbar>（vNode 名不是 ElHeader），自动判定永不命中，header 与 main
         会横向并列。显式指定后 class 为 el-container is-vertical，flex-direction 才是 column。 -->
    <el-container direction="vertical">
      <Topbar
        :collapsed="isCollapsed"
        :mobile="isMobileNav"
        :clock="clock"
        :gateway-switcher="sw"
        :title="currentTitle"
        :group="currentGroup"
        :hub-tab="hubTabLabel"
        :security-warnings="auth.securityWarnings"
        @toggle-collapse="toggleCollapse"
        @open-security="showSecurityDrawer = true"
        @logout="handleLogout"
        @change-password="() => router.push('/change-password')"
        @open-license="showLicenseDialog = true"
        @open-request-monitor="showRequestMonitor = true"
        @tier-change="onTierChange"
        @diagnostic-report="exportDiagnostic"
      />

      <!-- tabindex="-1"：skip-link 跳转后焦点落到这里，否则锚点跳转对键盘用户无效 -->
      <el-main id="main-content" class="main-content" tabindex="-1">
        <div class="main-inner">
          <!-- 页级 h1：非 Hub / 非自带页头的页面给读屏与文档大纲一个顶级标题；
               Hub 页的 h1 由 HubShell 按当前页签渲染、概览页由自身页头渲染，这里跳过以免同页出现两个 h1 -->
          <h1 v-if="!route.meta.hub && !route.meta.pageHeader" class="sr-only">{{ currentTitle }}</h1>
          <!-- 连通性横幅：断连时明确告知「页面数据是历史值」，避免现场误判为实时 -->
          <ConnectivityBanner
            :unreachable="gatewayUnreachable"
            :degraded="bannerDegraded"
            :components="bannerComponents"
            :queue-len="queueLen"
            :stale-seconds="staleSeconds"
            :fail-streak="failStreak"
            :retrying="connectivityRetrying"
            @retry="retryConnectivity"
          />
          <!-- 安全提示：默认凭据 / 匿名只读 / 开放 CORS / 全网卡监听。
               只在本机首次会话出现一次；此后风险状态由顶栏徽章常驻承担。
               N12: 原先把后端的长句风险项逐条平铺（4 条 ≈ 150px 高，直接吃掉首屏），
               这里收敛成一行「几项 + 短标签」，全文与处置入口进抽屉（顶栏徽章同一抽屉）。 -->
          <el-alert
            v-if="showSecurityNotice"
            class="security-notice"
            type="warning"
            show-icon
            @close="dismissSecurityNotice"
          >
            <template #title>
              <span class="security-notice-text" :title="auth.securityWarnings.join('；')">
                存在 {{ auth.securityWarnings.length }} 项安全风险：{{ securitySummary }}
              </span>
              <el-button link type="primary" class="security-notice-more" @click.stop="showSecurityDrawer = true">
                查看详情
              </el-button>
            </template>
          </el-alert>
          <router-view v-slot="{ Component, route }">
            <transition name="fade" mode="out-in">
              <!-- 页面级错误边界：单个页面崩了只降级至此页，侧栏顶栏不受影响。
                   key 用 path 而非 fullPath：query 变化（切 tab / 筛选 / 选设备）不算换页，
                   否则整页会被卸载重挂 —— 现场状态全丢，还会闪一下白屏。
                   keyed div 必须是 transition 的直接子节点：路由组件是异步 import（未解析时
                   渲染注释占位符），且 PageErrorBoundary 根级 v-if/v-else 编译为 Fragment，
                   二者都不是元素根节点 —— 直接放进 transition 会触发 Vue 告警并跳过过渡。
                   边界不挂 key：父 div 换页重建时边界随之重挂，错误态天然按页重置。 -->
              <div :key="route.path" class="route-page">
                <PageErrorBoundary :name="route.path" boundary="route">
                  <component :is="Component" />
                </PageErrorBoundary>
              </div>
            </transition>
          </router-view>
        </div>
      </el-main>
    </el-container>

    <GatewayScanDialog
      v-model="showScanDialog"
      :scanning="isScanning"
      :results="scanResults"
      :add-disabled="isSwitchingGateway"
      @add="sw.addDiscoveredGateway"
      @rescan="sw.performLanScan"
    />

    <!-- 授权管理：设备码复制 / .lic 导入（Topbar 徽章与用户菜单入口） -->
    <LicenseDialog v-model="showLicenseDialog" />

    <!-- 请求监视器：生产环境不常驻悬浮球，从用户菜单打开，仅打开期间采样 -->
    <RequestMonitor v-model:open="showRequestMonitor" />

    <!-- 键盘快捷键说明（? 键打开） -->
    <ShortcutHelpDialog v-model="showShortcuts" />

    <!-- 安全风险详情：顶栏徽章点开，列出全部风险条目并给出处理入口。
         不直接跳设置页，是因为不同风险要改的地方不同（密码 / 配置 / 绑定网卡）。 -->
    <el-drawer v-model="showSecurityDrawer" title="安全风险" size="420px">
      <p class="security-drawer-hint">当前网关存在以下配置风险，接入生产网络前建议逐项收敛：</p>
      <ul class="security-drawer-list">
        <li v-for="(w, i) in auth.securityWarnings" :key="i">{{ w }}</li>
      </ul>
      <div class="security-drawer-actions">
        <el-button type="primary" @click="gotoSecurityFix('/change-password')">修改默认密码</el-button>
        <el-button @click="gotoSecurityFix('/system/config')">去配置管理</el-button>
      </div>
    </el-drawer>
  </el-container>
</template>

<script setup lang="ts">
/**
 * Layout 薄壳
 *
 * 拆分（B.1 / B.2 / B.3）后的壳层只剩：
 *   - 容器骨架
 *   - 子组件装配（Sidebar / Topbar / GatewayScanDialog / LicenseDialog）
 *   - router-view 包装 PageErrorBoundary（路由级错误降级）
 *   - 跨子组件状态共享（折叠、弹窗显隐、登出/tier 切换等副作用）
 *
 * 所有数据/副作用逻辑已下沉到：
 *   - useLayoutClock    : 时钟、健康轮询、username、主题
 *   - useGatewaySwitcher: 网关切换、LAN 扫描
 *   - stores/tier, auth, gateway
 *   - 各页面组件 + PageErrorBoundary
 *
 * 本组件自身只持有"跨子组件状态"（isCollapsed）与全局快捷键。
 */
import { ref, computed, onMounted, onUnmounted, watch } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { abortAllInflight } from '@/api'
import { confirmAction } from '../utils/confirm'
import { useAuthStore } from '../stores/auth'
import { useTierStore, type Tier } from '../stores/tier'
import { findGroupNameByPath } from './layout/menu'
import { useLayoutClock } from '@/composables/layout/useLayoutClock'
import { useGatewaySwitcher } from '@/composables/layout/useGatewaySwitcher'
import { useHubContext } from '@/composables/core/useHubContext'
import { useDiagnosticExport } from '@/composables/system/useDiagnosticExport'
import { loadProtocolMeta } from '@/composables/devices/useProtocol'
import { scopedGet, scopedRemove, scopedSet } from '@/utils/gatewayStorage'
import Sidebar from './layout/Sidebar.vue'
import Topbar from './layout/Topbar.vue'
import GatewayScanDialog from './layout/GatewayScanDialog.vue'
import LicenseDialog from './layout/LicenseDialog.vue'
import ShortcutHelpDialog from './layout/ShortcutHelpDialog.vue'
import RequestMonitor from '../components/RequestMonitor.vue'
import PageErrorBoundary from '../components/PageErrorBoundary.vue'
import ConnectivityBanner from '../components/ConnectivityBanner.vue'

const route = useRoute()
const router = useRouter()
const auth = useAuthStore()
const tierStore = useTierStore()

// ===== 跨子组件共享的 composables =====
const clock = useLayoutClock()
const sw = useGatewaySwitcher()
// 模板直接绑定的响应式状态需解构到顶层：普通对象上的 Ref 属性在模板中不会自动解包
const { showScanDialog, isScanning, scanResults, isSwitchingGateway } = sw
// 连通性横幅同理（clock 是普通对象，嵌套 Ref 不会在模板自动解包）
const { gatewayUnreachable, gatewayDegraded, degradedComponents, staleSeconds, failStreak, queueLen } = clock

/**
 * 告警降扰：断连续传页（/data?tab=data-buffering）本身就是看队列积压的页面，
 * 顶部横幅再报一次「队列积压」属于同页重复强调——过滤掉该项；
 * 若过滤后没有其他异常组件，整条横幅不再显示。
 * 老后端无 components 明细时（空数组）保持原逻辑，不误隐藏。
 */
const isBufferingTab = computed(() => route.path === '/data' && route.query.tab === 'data-buffering')
const bannerComponents = computed(() =>
  isBufferingTab.value
    ? degradedComponents.value.filter((c) => !c.includes('队列积压'))
    : degradedComponents.value,
)
const bannerDegraded = computed(
  () =>
    gatewayDegraded.value &&
    !(isBufferingTab.value && degradedComponents.value.length > 0 && bannerComponents.value.length === 0),
)

// 横幅「立即重试」：复用健康检查，期间按钮保持 loading 防重复点击
const connectivityRetrying = ref(false)
async function retryConnectivity() {
  if (connectivityRetrying.value) return
  connectivityRetrying.value = true
  try {
    await clock.checkHealth()
  } finally {
    connectivityRetrying.value = false
  }
}

// ===== 折叠状态（受控） =====
const isCollapsed = ref(false)
/**
 * 窄屏（≤768px）改用抽屉式导航：此前窄屏直接把侧栏折成 64px 图标列，
 * 分组名与二级菜单名全不可见，且探不出分组层级。抽屉保留完整 220px 菜单，
 * 由顶栏汉堡按钮打开、点遮罩或切路由后关闭。
 */
const MOBILE_BREAKPOINT = 768
const isMobileNav = ref(false)
const mobileNavOpen = ref(false)
let manualCollapse = false
function toggleCollapse() {
  if (isMobileNav.value) {
    mobileNavOpen.value = !mobileNavOpen.value
    return
  }
  isCollapsed.value = !isCollapsed.value
  manualCollapse = true
}
function applyBreakpoint() {
  isMobileNav.value = window.innerWidth <= MOBILE_BREAKPOINT
  if (isMobileNav.value) {
    // 抽屉模式下侧栏必须展开（要看到分组名），折叠态只服务桌面端
    isCollapsed.value = false
    mobileNavOpen.value = false
    return
  }
  if (manualCollapse) return
  isCollapsed.value = false
}
function onResize() {
  applyBreakpoint()
}

// ===== 全局快捷键 =====
const showShortcuts = ref(false)
const showRequestMonitor = ref(false)

/** 键位监听会挂在 window 上，必须避开输入态，否则在搜索框里打 "?" 会被当成快捷键 */
function isTypingTarget(target: EventTarget | null): boolean {
  const el = target as HTMLElement | null
  if (!el) return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable
}

// Ctrl+Shift+M 开关请求监视器：调试工程师高频操作，比从用户菜单深入快得多。
// 与浏览器快捷键无冲突（Ctrl+Shift+M 非 Chrome/Firefox 保留组合）。
// "?" 打开快捷键说明（Shift+/ 在多数键盘布局下产生 "?"）。
function onGlobalKeydown(e: KeyboardEvent) {
  if ((e.ctrlKey || e.metaKey) && e.shiftKey && (e.key === 'm' || e.key === 'M')) {
    e.preventDefault()
    showRequestMonitor.value = !showRequestMonitor.value
    return
  }
  if (e.key === '?' && !e.ctrlKey && !e.metaKey && !e.altKey && !isTypingTarget(e.target)) {
    e.preventDefault()
    showShortcuts.value = true
  }
}
// 授权管理弹窗（Topbar 徽章 / 用户菜单入口）
const showLicenseDialog = ref(false)

// ===== 面包屑数据 =====
const currentTitle = computed(() => (route.meta.title as string) || '')
// 抽屉模式下选中菜单即收起：否则 220px 抽屉会一直盖住刚打开的页面
watch(
  () => route.fullPath,
  () => { mobileNavOpen.value = false },
)
const currentGroup = computed(() => findGroupNameByPath(route.path))
// Hub 页当前 tab（HubShell 写入）：用于面包屑第三层「分组 / 页面 / 页签」
const { hubTabLabel } = useHubContext()

// ===== 诊断报告（顶栏用户菜单入口）=====
// 与系统总览页共用同一实现：网关异常时无需先进入系统设置才能取证
const { exportDiagnostic } = useDiagnosticExport()

// ===== 登出（顶层副作用，避免 Topbar 直接持有 router/store） =====
async function handleLogout() {
  if (!await confirmAction({ message: '确定退出登录？' })) return
  abortAllInflight('logout')
  auth.logout()
  try { scopedRemove('gw_username') } catch { /* ignore */ }
  router.push('/login')
}

// ===== DEV tier 切换：跳回 dashboard 避免停留在无权限页 =====
function onTierChange(next: Tier) {
  tierStore.setTier(next)
  if (route.path !== '/overview') router.push('/overview')
}

// ===== 安全提示（默认凭据 / 匿名只读 / 开放 CORS / 全网卡监听）=====
// 降级为「顶栏徽章常驻 + 首次会话展开一次横幅」：
// 横幅只在从未展示过的机器上出现一次（写 localStorage，展示即落库），
// 此后风险条目随配置长期存在也只在徽章上体现，不再每会话占掉首屏 ~50px。
// 故意不用 sessionStorage —— 那样每次新会话都会重新冒出来，正是要修掉的问题。
const SECURITY_BANNER_KEY = 'gw_security_banner_shown'
const securityBannerShown = ref(scopedGet(SECURITY_BANNER_KEY) === '1')
const showSecurityDrawer = ref(false)
const showSecurityNotice = computed(
  () => !securityBannerShown.value && auth.securityWarnings.length > 0,
)
/** 后端风险项是完整长句（每条 40~60 字），横幅只放得下一行，这里按关键词映射成短标签。
 *  映射不中时回落原文，由 CSS 省略号兜底（全文始终在 title 提示与抽屉里）。 */
const SECURITY_TAGS: Array<[RegExp, string]> = [
  [/默认凭据|默认密码/, '默认密码'],
  [/匿名只读/, '匿名只读'],
  [/跨域/, 'CORS 未限制'],
  [/所有网卡|全网卡/, '全网卡监听'],
]
function securityTagOf(text: string): string {
  const hit = SECURITY_TAGS.find(([re]) => re.test(text))
  return hit ? hit[1] : text
}
const securitySummary = computed(() => auth.securityWarnings.map(securityTagOf).join('、'))
function dismissSecurityNotice() {
  securityBannerShown.value = true
  try { scopedSet(SECURITY_BANNER_KEY, '1') } catch { /* ignore */ }
}
/** 风险处置入口：关掉抽屉再跳转，避免抽屉盖住刚打开的页面 */
function gotoSecurityFix(path: string) {
  showSecurityDrawer.value = false
  router.push(path)
}

// ===== 生命周期 =====
onMounted(() => {
  applyBreakpoint()
  // 横幅已经展示过（不管用户是否点了关闭）就落库：首次会话看得到，之后只留顶栏徽章
  if (showSecurityNotice.value) {
    try { scopedSet(SECURITY_BANNER_KEY, '1') } catch { /* ignore */ }
  }
  window.addEventListener('resize', onResize)
  window.addEventListener('keydown', onGlobalKeydown)
  // 首次拉网关列表（Tauri 下有效）
  sw.refreshGateways()
  // 拉取 license 快照（刷新保活场景；登录路径由 auth store 触发）
  void tierStore.refreshFromLicense()
  // 协议清单/默认超时/地址提示以后端注册表为准（失败时保留内置兜底）
  void loadProtocolMeta()
})
onUnmounted(() => {
  window.removeEventListener('resize', onResize)
  window.removeEventListener('keydown', onGlobalKeydown)
})
</script>

<style scoped>
.security-notice { margin-bottom: 12px; border-radius: var(--radius-table); }
/* 标题行改为单行 flex：摘要文本可省略，右侧「查看详情」不被挤压 */
.security-notice :deep(.el-alert__title) {
  display: flex;
  align-items: center;
  gap: 10px;
  min-width: 0;
}
.security-notice-text {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.security-notice-more { flex: none; padding: 0; height: auto; font-size: inherit; }
.security-drawer-hint { margin: 0 0 12px; color: var(--text-secondary); font-size: 0.8125rem; line-height: 1.7; }
.security-drawer-list { margin: 0 0 20px; padding-left: 18px; font-size: 0.875rem; line-height: 1.9; }
.security-drawer-actions { display: flex; gap: 8px; }
</style>

<style scoped src="./Layout.css" />