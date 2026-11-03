<template>
  <el-aside :width="collapsed ? '64px' : '220px'" class="sidebar" :class="{ collapsed }">
    <div class="logo">
      <div class="logo-icon">
        <!-- 与登录页同一份品牌标识（BrandMark），保证两处造型完全一致 -->
        <BrandMark :size="24" />
      </div>
      <span v-show="!collapsed" class="logo-text">工业网关</span>
    </div>
    <div v-show="!collapsed" class="menu-search">
      <el-input v-model="menuKeyword" size="small" placeholder="搜索菜单 / 功能..." aria-label="搜索菜单" clearable>
        <template #prefix><el-icon><Search /></el-icon></template>
      </el-input>
    </div>
    <div v-if="menuKeyword && searchResults.length === 0" class="menu-empty" role="status">
      无匹配菜单
      <span>换个关键词试试（页面名、功能名、页签均可）</span>
    </div>
    <el-menu
      ref="menuRef"
      :default-active="activeMenu"
      :default-openeds="defaultOpeneds"
      :collapse="collapsed"
      :collapse-transition="true"
      router
      @select="onMenuSelect"
      background-color="var(--dark-bg)"
      text-color="var(--dark-text-secondary)"
      active-text-color="var(--dark-accent)"
    >
      <template v-if="menuKeyword">
        <el-menu-item
          v-for="item in searchResults"
          :key="item.id"
          :index="item.target"
          @mouseenter="prefetch(item.target)"
          @focus="prefetch(item.target)"
        >
          <el-icon><component :is="item.icon" /></el-icon>
          <template #title>
            <span>{{ item.title }}</span>
            <!-- 补「业务域 › 功能名」路径：搜索命中里既有菜单项也有页签级条目，单看标题分不清 -->
            <span class="menu-item-path">{{ item.pathLabel }}</span>
          </template>
        </el-menu-item>
      </template>
      <template v-else>
        <el-sub-menu v-for="group in filteredMenuGroups" :key="group.name" :index="group.name">
          <template #title>
            <el-icon><component :is="group.icon" /></el-icon>
            <span>{{ group.name }}</span>
          </template>
          <el-menu-item
            v-for="item in group.items"
            :key="item.path"
            :index="item.path"
            :title="item.description"
            @mouseenter="prefetch(item.path)"
            @focus="prefetch(item.path)"
          >
            <el-icon><component :is="item.icon" /></el-icon>
            <template #title>{{ item.title }}</template>
          </el-menu-item>
        </el-sub-menu>
      </template>
    </el-menu>
  </el-aside>
</template>

<script setup lang="ts">
/**
 * Layout 侧栏
 *
 * 负责：logo + 菜单搜索 + 折叠菜单（响应路由高亮）。
 * 通过 props 与父组件握手：collapsed 双向绑定（v-model），
 * 暴露 defaultOpeneds 计算结果由 route 变化驱动。
 *
 * 不在本组件内维护：
 *   - 折叠状态（父组件管，本组件只接收状态）
 *   - 网关/健康/主题等其它顶栏事
 */
import { ref, computed, watch, nextTick } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { Search } from '@element-plus/icons-vue'
import BrandMark from '@/components/BrandMark.vue'
import { prefetchRoute } from '@/router/prefetch'
import { useFilteredMenuGroups, useNavIndex, findGroupNameByPath } from './menu'

defineProps<{
  /** 是否折叠（受控） */
  collapsed: boolean
}>()
defineEmits<{
  (e: 'update:collapsed', v: boolean): void
}>()

const route = useRoute()
const router = useRouter()
/**
 * 悬停/聚焦菜单项时预取目标页 chunk。
 * 点击前的悬停通常有 150ms 以上，足够触发下载，切页时不必再等 chunk。
 */
function prefetch(path: string) {
  prefetchRoute(router, path)
}
const filteredMenuGroups = useFilteredMenuGroups()
// 搜索索引摊平了「菜单项」与「Hub 页签级深链」，
// 因此侧栏搜索也能命中「报文回放」「从站扫描」等只存在于 ?tab= 深链里的页签。
const navIndex = useNavIndex()

const menuKeyword = ref('')
// el-menu 实例引用：default-openeds 只在挂载时读取一次（见 element-plus menu.mjs:182），
// 路由变化后需主动调 open() 展开当前分组，否则从搜索跳转的项会藏在折叠分组里。
const menuRef = ref<{ open: (index: string) => void } | null>(null)

const searchResults = computed(() => {
  const kw = menuKeyword.value.trim().toLowerCase()
  if (!kw) return []
  return navIndex.value.flatMap((c) => {
    const hit = c.title.toLowerCase().includes(kw)
      || (c.subtitle ?? '').toLowerCase().includes(kw)
      || (c.alias ?? '').toLowerCase().includes(kw)
      || c.target.toLowerCase().includes(kw)
    if (!hit) return []
    return [{ id: c.id, title: c.title, icon: c.icon, target: c.target, pathLabel: pathLabelOf(c.target, c.title) }]
  })
})

/**
 * 搜索结果里的层级路径「业务域 › 功能名」。
 * 搜索命中的多是页签级条目，只显示标题时
 * 「数据质量」「设备管理」这类同名功能无法分辨属于哪个业务域。
 */
function pathLabelOf(target: string, title: string): string {
  const group = findGroupNameByPath(target.split('?')[0] ?? '')
  return group ? `${group} › ${title}` : title
}

const activeMenu = computed(() => {
  // 搜索态下渲染的菜单项 :index 是完整 target（含 ?tab=），按 fullPath 匹配才能命中；
  // 分组态下菜单项 :index 是纯路径，而 HubShell 挂载后会把 ?tab= 写进 URL，
  // EP 按 index 与 activeIndex 精确匹配，fullPath 会让 16/25 个菜单项整栏失去高亮
  // （如 /devices?tab=devices ≠ 菜单 index /devices），因此分组态一律按 route.path 匹配。
  if (menuKeyword.value) {
    const hit = navIndex.value.find((c) => c.target === route.fullPath)
    if (hit) return hit.target
  }
  return route.path
})

/** 选中搜索结果即退出搜索态，恢复分组菜单 */
function onMenuSelect() {
  menuKeyword.value = ''
}

const defaultOpeneds = computed(() => {
  // 复用 menu 层的单一来源（与面包屑同一套匹配规则），
  // 避免侧栏与面包屑对同一路径给出不同分组（如 /system/encryption）。
  const name = findGroupNameByPath(route.path)
  // 组名必须在当前可见分组里存在：过滤后组可能被裁掉，
  // 用已不存在的组名展开会落空（侧栏看不到任何展开项）。
  const groups = filteredMenuGroups.value
  const visible = groups.some((g) => g.name === name) ? name : ''
  return visible ? [visible] : (groups[0]?.name ? [groups[0].name] : ['仪表盘'])
})

// breadcrumb 当前分组（暴露给 Layout 顶部面包屑用）
const currentGroup = computed(() => findGroupNameByPath(route.path))

// 路由切换后主动展开当前项所在分组（default-openeds 不响应式）
watch(
  () => route.path,
  () => {
    const g = currentGroup.value
    if (!g) return
    // 等子菜单注册完成后再 open，否则 subMenus 里找不到该分组
    nextTick(() => menuRef.value?.open(g))
  },
)

defineExpose({ activeMenu, defaultOpeneds, currentGroup, searchResults, menuKeyword })
</script>

<style src="./Sidebar.css" />