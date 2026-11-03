<template>
  <div class="hub-shell">
    <!-- 左侧分组导航：全站 Hub 统一使用这一种页内导航范式。
         之前按页签数量在「左侧分组」和「顶部横向标签条」之间切换，同一个产品
         出现两套二级导航（页签少时还要在两者之间迁移），用户学习成本与视觉
         一致性都受影响；现固定为左侧分组，页签少时只是导航更短。 -->
    <nav class="hub-nav" aria-label="功能分组">
      <div v-for="g in groups" :key="g.name" class="hub-nav-group">
        <!-- 9 页签 Hub 的左侧导航很长：分组可折叠，收起后整屏只留高频入口。
             折叠状态按路由持久化，二次进入保持用户上次的组织方式。 -->
        <button
          v-if="g.label"
          type="button"
          class="hub-nav-group-label"
          :class="{ collapsed: isGroupCollapsed(g.name) }"
          :aria-expanded="!isGroupCollapsed(g.name)"
          :aria-controls="groupBodyId(g.name)"
          :title="isGroupCollapsed(g.name) ? `展开分组：${g.label}` : `收起分组：${g.label}`"
          @click="toggleGroup(g.name)"
        >
          <el-icon class="hub-nav-caret"><ArrowRight /></el-icon>
          <span>{{ g.label }}</span>
        </button>
        <div :id="groupBodyId(g.name)" class="hub-nav-group-body" role="group" :aria-label="g.label || '功能'">
          <!-- 用 v-if 而非 v-show：折叠后页签要真正移出 DOM，
               否则键盘 Tab 仍会聚焦到看不见的按钮 -->
          <template v-if="!g.label || !isGroupCollapsed(g.name)">
            <button
              v-for="tab in g.tabs"
              :key="tab.name"
              type="button"
              class="hub-nav-item"
              :class="{ active: tab.name === activeTab }"
              :aria-current="tab.name === activeTab ? 'page' : undefined"
              @click="selectTab(tab.name)"
            >
              <span>{{ tab.label }}</span>
            </button>
          </template>
        </div>
      </div>
    </nav>

    <!-- 右侧内容：只渲染当前 tab（不常驻），切走即卸载 -->
    <div class="hub-body">
      <!-- 页头显示 Hub 名（= 菜单名 = meta.title）+ 分组小标题 + 副标题。
           当前页签名由左侧导航的选中态承担，页内区块标题统一用 h2，不再承载页面名。 -->
      <header class="hub-body-header">
        <!-- 页内分组小标题：左侧导航滚动或折叠后分组名会看不见，
             这里把当前页签所属分组固定显示在标题上方，保住「分组 / 页签」的层级 -->
        <p v-if="groupText" class="hub-body-eyebrow">{{ groupText }}</p>
        <!-- 页面 H1 统一取路由 meta.title（= 菜单名 = Hub 名）：左侧导航已标出当前页签，
             这里再写一遍页签名会让「同一页两个名字」，也与顶栏面包屑对不上 -->
        <h1 class="hub-body-title">{{ hubTitle || currentTab?.label }}</h1>
        <p v-if="currentTab?.subtitle" class="hub-body-subtitle">{{ currentTab.subtitle }}</p>
      </header>

      <!-- 可选工具栏（如协议调试台的「调试目标」条）：放在页头与内容之间，
           没有传入插槽内容的 Hub 渲染结果不变 -->
      <slot name="toolbar" />

      <div v-if="currentTab" class="hub-content">
        <!-- 需要保留现场的 Hub（如报文调试）用 KeepAlive 缓存已访问 tab；
             普通 Hub 保持“切走即卸载”，避免后台定时器/图表长期常驻。 -->
        <KeepAlive v-if="keepAlive" :max="5">
          <component :is="currentTab.component" :key="currentTab.name" />
        </KeepAlive>
        <component v-else :is="currentTab.component" :key="currentTab.name" />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * HubShell：所有聚合页（SystemHub/OpsHub/UplinkHub/ConfigHub 等）的统一外壳。
 *  - 接收 tabs 元数据数组（name/label/requireAdmin/component/subtitle/group）
 *  - v2.0：调试/分析功能全档位免费，可见性只按 requireAdmin（用户角色）过滤
 *  - 同步 URL ?tab= 参数：URL 指定 tab 不可见时回退首个可见 tab。
 *
 * ## 为什么改成分组 + 受控卸载
 *
 * 旧实现是 `el-tabs` + 每个 pane 标 `lazy`。但 element-plus 的 `lazy` 语义是
 * 「首次激活才渲染，**之后永久常驻**」（`loaded` 置 true 后不复位，只用 v-show 隐藏）。
 * 于是用户点过的每个 tab 都长期挂载：轮询定时器、echarts 实例、WebSocket 监听
 * 全部继续运行。SystemHub / ConfigHub 各有 9 个 tab，点一遍就是 9 套后台任务常驻。
 *
 * 现在改为：统一使用左侧分组导航，只渲染 `activeTab` 一个组件，切走即卸载
 * （onUnmounted 正常执行，定时器/图表/连接都能释放）。
 * 全站只有这一套页内导航范式：不再按页签数量在「左侧分组」与「顶部标签条」
 * 之间切换（两套范式会让同一产品的二级导航随页签数量变形）。
 *
 * 未保存数据的处理：`handleBeforeLeave` 保持原有的脏检查，且在卸载前拦截——
 * 弹框返回 false 时不会发生卸载，数据不会静默丢失。
 *
 * 使用方只需声明 tabs 数组：
 *   <HubShell :tabs="[
 *     { name: 'overview', label: '网关信息', group: 'daily', component: SystemOverview },
 *   ]" :group-labels="{ daily: '日常' }" fallback-tab="overview" />
 */
import { ref, computed, watch, onMounted, onUnmounted } from 'vue'
import type { Component } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { ArrowRight } from '@element-plus/icons-vue'
import { useAuthStore } from '@/stores/auth'
import { confirmDiscardDirty } from '@/utils/dirtyGuard'
import { setHubTabLabel } from '@/composables/core/useHubContext'
import { globalGet, globalSet } from '@/utils/gatewayStorage'

export interface HubTab {
  name: string
  label: string
  requireAdmin?: boolean
  component: Component
  /** 副标题：分组模式下显示在内容区页头，帮助用户理解这一页做什么 */
  subtitle?: string
  /** 所属分组名；不填视为「默认」单分组（不显示分组标题，仅列页签） */
  group?: string
  /**
   * 置顶标记：高频操作 tab（如 RuntimeConfig）声明后，在 visibleTabs 排序时排到分组最前。
   * 不设则按声明顺序。
   */
  pin?: boolean
}

/** 未显式指定分组时的分组名，保证单分组场景也能走分组渲染分支判断 */
const DEFAULT_GROUP = '__default__'

const props = withDefaults(defineProps<{
  tabs: HubTab[]
  /** URL 无 tab 且无可见 tab 时使用的兜底名 */
  fallbackTab: string
  /** group 名 → 展示文案；未给出的分组用 group 名本身 */
  groupLabels?: Record<string, string>
  /**
   * 是否缓存已访问的 tab（默认 false）。
   * 仅用于会话型调试页（抓包缓冲、单步队列、回放进度），切换 tab 不再丢现场；
   * 普通 Hub 保持卸载以释放轮询/图表资源。
   */
  keepAlive?: boolean
  /**
   * 首次进入时默认收起的分组名（「常用优先」）：
   * 13 页签的 SystemHub 一次铺开会把常用项挤出首屏，低频分组（高级/固件与证书）默认收起，
   * 首屏只留高频入口。用户手动展开后的状态仍按路由持久化，下次进入尊重用户选择。
   */
  defaultCollapsedGroups?: string[]
}>(), {
  groupLabels: () => ({}),
  keepAlive: false,
  defaultCollapsedGroups: () => [],
})

const auth = useAuthStore()

// v2.0：不再按 tier 过滤（调试/分析功能全免费），仅保留管理员角色过滤
// 排序：pinned 置顶 → 其余保持声明顺序（Array.sort 稳定）
const visibleTabs = computed(() =>
  props.tabs
    .filter((t) => !t.requireAdmin || auth.roleId === 'admin')
    .sort((a, b) => Number(!!b.pin) - Number(!!a.pin)),
)

/** 按 group 归并，保持 tabs 声明顺序 */
const groups = computed(() => {
  const map = new Map<string, HubTab[]>()
  for (const t of visibleTabs.value) {
    const key = t.group ?? DEFAULT_GROUP
    const list = map.get(key)
    if (list) list.push(t)
    else map.set(key, [t])
  }
  return [...map.entries()].map(([name, tabs]) => ({
    name,
    // 默认分组不显示标题：单分组 Hub 只列页签，避免把内部占位名 "__default__" 暴露给用户
    label: name === DEFAULT_GROUP ? '' : props.groupLabels[name] ?? name,
    tabs,
  }))
})

const route = useRoute()
const router = useRouter()

/**
 * 父级（Hub / 菜单）名：取路由 meta.title，与侧栏菜单项同名。
 *
 * H1 统一用这个名字：菜单名 = meta.title = Hub 名 = H1，
 * 页签名（具体视图）只出现在左侧分组导航上。
 * 这样从菜单点进来，页面标题与菜单、浏览器标题、面包屑第二层完全一致。
 */
const hubTitle = computed(() => (route.meta?.title as string) || '')

/**
 * 当前页签所属分组的展示名（页头小标题）。
 * - 未声明 group（单分组退化场景）不显示，避免冒出无意义的「默认」；
 * - 与页名相同也不显示：如日志 Hub 的分组「运行日志」就等于菜单名，重复一次只是噪声。
 */
const groupText = computed(() => {
  const g = currentTab.value?.group
  if (!g || g === DEFAULT_GROUP) return ''
  const label = props.groupLabels[g] ?? g
  return label === hubTitle.value ? '' : label
})

/**
 * 分组折叠状态：按路由持久化到 localStorage。
 * 9 页签 Hub 一次铺开 9 行导航会挤占内容区，收起低频分组后只需 2~3 行。
 */
// 默认收起的分组打底，再叠加用户已持久化的选择（用户的显式展开要能覆盖默认值）
const collapsedGroups = ref<Record<string, boolean>>({
  ...defaultCollapsed(),
  ...loadCollapsedGroups(),
})

function defaultCollapsed(): Record<string, boolean> {
  const out: Record<string, boolean> = {}
  for (const g of props.defaultCollapsedGroups) out[g] = true
  return out
}

function collapseStorageKey(): string {
  return `gw-hub-collapsed:${route.path}`
}

function loadCollapsedGroups(): Record<string, boolean> {
  try {
    const raw = globalGet(collapseStorageKey())
    const parsed = raw ? JSON.parse(raw) : null
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, boolean>) : {}
  } catch {
    return {}
  }
}

function saveCollapsedGroups(): void {
  try {
    globalSet(collapseStorageKey(), JSON.stringify(collapsedGroups.value))
  } catch {
    // 隐私模式 / 配额满：折叠仍可用，只是不记住，无需打扰用户
  }
}

function isGroupCollapsed(name: string): boolean {
  return collapsedGroups.value[name] === true
}

function toggleGroup(name: string): void {
  collapsedGroups.value = { ...collapsedGroups.value, [name]: !isGroupCollapsed(name) }
  saveCollapsedGroups()
}

/** 稳定 id：给分组内容区提供 aria-controls 目标 */
function groupBodyId(name: string): string {
  return `hub-group-${name}`
}

function pickInitialTab(): string {
  const q = route.query.tab as string | undefined
  if (q && visibleTabs.value.some((t) => t.name === q)) return q
  return visibleTabs.value[0]?.name ?? props.fallbackTab
}

const activeTab = ref(pickInitialTab())

const currentTab = computed(() =>
  visibleTabs.value.find((t) => t.name === activeTab.value),
)

// 未保存更改守卫：tab 切换前检查脏注册表（同路由 query 变化不触发 router.beforeEach）
// Bug-W6 (fix)：仅清理当前路由 path 的脏标记，保留其他 tab/scope 的未保存数据。
async function handleBeforeLeave(): Promise<boolean> {
  return await confirmDiscardDirty(route.path)
}

/**
 * 页签切换入口（侧栏项与顶部标签共用）：先过脏检查，确认后才切。
 * 显式 await，返回 false 时绝不改变 activeTab —— 组件也就不会被卸载。
 */
async function selectTab(name: string): Promise<void> {
  if (name === activeTab.value) return
  if (!(await handleBeforeLeave())) return
  activeTab.value = name
}

watch(activeTab, (tab) => {
  if (tab !== route.query.tab) router.replace({ query: { ...route.query, tab } })
})

// 顶栏面包屑补一层当前 tab；Hub 卸载时清空，避免离开后残留
watch(currentTab, (t) => setHubTabLabel(t?.label ?? ''), { immediate: true })
onUnmounted(() => setHubTabLabel(''))

// 地址栏不变量：留在本 Hub 时 URL 上的 tab 必须就是界面上显示的 tab
const hubPath = route.path

/** 把 URL 上的 tab 改写为当前实际显示的 tab（幂等，已一致时不触发 replace） */
function syncUrlTab(): void {
  if (route.path !== hubPath) return
  if (route.query.tab === activeTab.value) return
  router.replace({ query: { ...route.query, tab: activeTab.value } })
}

/**
 * URL 驱动的切换（浏览器前进/后退、侧栏或外部链接带 ?tab=）同样必须先过脏检查。
 * 否则 activeTab 直接改值会导致旧 tab 组件被卸载，未保存数据静默丢失。
 * 用户拒绝丢弃时把地址栏拉回当前 tab，保证 URL 与界面一致。
 */
watch(() => route.query.tab, async (tab) => {
  if (typeof tab !== 'string' || tab === activeTab.value) return
  // 无效 tab（拼错、旧链接、角色切换后已不可见的页签）：界面会静默回退到首个可见
  // tab，此时地址栏必须跟着改，否则刷新/分享会一次次落回同一个无效值
  if (!visibleTabs.value.some((t) => t.name === tab)) {
    syncUrlTab()
    return
  }
  if (await confirmDiscardDirty(route.path)) {
    activeTab.value = tab
  } else {
    router.replace({ query: { ...route.query, tab: activeTab.value } })
  }
})

// 首次进入就带着无效 tab：watch 不会为初始值触发，这里补一次同步
onMounted(() => {
  if (typeof route.query.tab === 'string') syncUrlTab()
})

/**
 * 地址栏不变量：留在本 Hub 时 URL 必须带 ?tab=。
 * 页内其他状态（如点位页选设备）会用 useQueryState 重写 query，而它只保留已有 key，
 * 若进页时 URL 本就没有 tab，重写后 tab 依然缺席 —— 刷新或分享链接会丢掉页签定位。
 * 这里统一兜底补回；route.path 已变则说明正在离开本页，绝不能把 tab 写到新路由上。
 */
watch(() => route.fullPath, () => {
  if (route.path !== hubPath) return
  if (typeof route.query.tab === 'string') return
  if (visibleTabs.value.length === 0) return
  router.replace({ query: { ...route.query, tab: activeTab.value } })
})
// 可见 tab 集合变化后（如角色切换），若当前 tab 不可见则回退到第一个可见 tab
watch(visibleTabs, (list) => {
  if (!list.some((t) => t.name === activeTab.value)) {
    activeTab.value = list[0]?.name ?? props.fallbackTab
  }
})
</script>

<style scoped>
.hub-shell {
  display: flex;
  gap: 16px;
  align-items: flex-start;
}
.hub-nav {
  flex: 0 0 152px;
  display: flex;
  flex-direction: column;
  gap: 14px;
  position: sticky;
  top: 0;
  /* 与侧栏区分：细竖线 + 更弱的文字色，表明这是页内二级导航而非第二个侧栏 */
  border-right: 1px solid var(--border-light);
  padding-right: 12px;
  /* 导航自身可滚动：9 个页签的 Hub（SystemHub / ConfigHub / DataPipelineHub）
   * 全展开时导航高约 500px，1366×768 视口下底部项会被裁掉且滚不到。
   * max-height 取视口高度减去上下留白，overflow-y 让长导航能滚。
   * 参考 src/views/layout/Sidebar.css 的 .sidebar .el-menu 同一做法。 */
  max-height: calc(100vh - 96px);
  overflow-y: auto;
  overflow-x: hidden;
  /* 细滚动条：窄栏里默认滚动条太宽会挤掉文字 */
  scrollbar-width: thin;
}
.hub-nav::-webkit-scrollbar { width: 6px; }
.hub-nav::-webkit-scrollbar-thumb {
  background: var(--el-border-color-darker);
  border-radius: 3px;
}
.hub-nav::-webkit-scrollbar-track { background: transparent; }
.hub-nav-group { display: flex; flex-direction: column; gap: 2px; }
.hub-nav-group-label {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  padding: 5px 8px;
  border: 0;
  border-radius: var(--radius-small);
  background: transparent;
  font: inherit;
  font-size: 0.75rem;
  color: var(--text-secondary);
  letter-spacing: 0.04em;
  text-align: left;
  cursor: pointer;
}
.hub-nav-group-label:hover { background: var(--el-fill-color-lighter); color: var(--text-regular); }
.hub-nav-caret {
  font-size: 12px;
  transform: rotate(90deg);
  transition: transform 0.18s ease;
}
.hub-nav-group-label.collapsed .hub-nav-caret { transform: rotate(0deg); }
.hub-nav-group-body { display: flex; flex-direction: column; gap: 2px; }
.hub-nav-item {
  appearance: none;
  border: 0;
  /* 常驻 3px 透明左边框：激活时只换颜色，不产生文字位移 */
  border-left: 3px solid transparent;
  background: transparent;
  display: flex;
  align-items: center;
  gap: 6px;
  text-align: left;
  padding: 8px 10px 8px 9px;
  border-radius: 0 var(--radius-small) var(--radius-small) 0;
  font-size: 0.875rem;
  color: var(--text-secondary);
  cursor: pointer;
  line-height: 1.4;
}
.hub-nav-item:hover { background: var(--el-fill-color-light); color: var(--text-regular); }
.hub-nav-item.active {
  background: var(--el-color-primary-light-9);
  color: var(--el-color-primary);
  border-left-color: var(--el-color-primary);
  font-weight: 600;
}

.hub-body { flex: 1; min-width: 0; }
.hub-body-header { margin-bottom: 16px; }
/* 页头小标题（当前页签所属分组）：层级靠字号与色阶拉开，不额外加分隔线 */
.hub-body-eyebrow {
  margin: 0 0 4px;
  font-size: 0.75rem;
  color: var(--text-secondary);
  letter-spacing: 0.04em;
}
.hub-body-title { margin: 0; font-size: 1.125rem; font-weight: 600; }
/* 副标题固定单行：过长时省略，避免把首屏高度继续往上推 */
.hub-body-subtitle {
  margin: 6px 0 0;
  font-size: 0.8125rem;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.hub-content { min-width: 0; }

/* 中等窄屏（1280px）：左侧分组导航紧凑化，为内容区腾出空间。
 * 1366×768 + 125% 缩放下有效视口 ~1093px，HubShell nav 从 168px → 124px，
 * 内容区多 44px 对表格横向滚动缓解明显。 */
@media (max-width: 1280px) {
  .hub-shell { gap: 10px; }
  .hub-nav { flex-basis: 124px; min-width: 124px; }
  .hub-nav-group-label { padding: 6px 8px; font-size: 0.75rem; }
  .hub-nav-item { padding: 7px 8px; font-size: 0.8125rem; border-radius: var(--radius-small); }
}

/* 中等偏窄屏：副标题先让位，优先保证标题与首行业务内容可见 */
@media (max-width: 1024px) {
  .hub-body-subtitle { display: none; }
}

/* 窄屏：左侧分组导航退化为顶部横向滚动条，避免挤压内容区 */
@media (max-width: 768px) {
  .hub-shell { flex-direction: column; }
  .hub-nav {
    flex: none;
    width: 100%;
    flex-direction: row;
    gap: 12px;
    overflow-x: auto;
    /* 窄屏改为横向排列，纵向不再需要限高；竖分隔线同步换成下划线 */
    max-height: none;
    overflow-y: hidden;
    padding: 0 0 4px;
    border-right: 0;
    border-bottom: 1px solid var(--border-light);
  }
  .hub-nav-group { flex-direction: row; align-items: center; gap: 6px; }
  /* 横向滚动条上分组标题与页签同排；折叠仍生效，条目收起后只留标题 */
  .hub-nav-group-label { width: auto; padding: 4px 6px; white-space: nowrap; }
  .hub-nav-group-body { flex-direction: row; gap: 6px; }
  .hub-nav-item { white-space: nowrap; }
}
</style>