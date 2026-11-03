/**
 * 全局 Vue Router 类型增强（在 tsconfig include 路径下自动加载，无需 import）。
 *
 * 设计：
 * - `requireAdmin` 供 admin 守卫读取（直接写在 routes/index.ts 的 meta 中）；
 * - `public` / `title` 与原始 router 一致；
 * - `loadError` / `loadErrorFrom` 供 /load-failed 兜底页使用。
 *
 * 注意：v2.0 起路由不再携带 `tier`（调试/分析功能全档位免费，tier 不参与访问控制），
 * 故此处不再增强 RouteRecordNormalized / 不定义 TierRouteRecord。
 *
 * 重要：这个文件是 .d.ts ambient declaration，依赖 tsconfig include 自动包含。
 */
declare module 'vue-router' {
  interface RouteMeta {
    /** 页面标题（显示在浏览器标签和侧边栏） */
    title?: string
    /** 公开页面，无需登录即可访问（如登录页） */
    public?: boolean
    /**
     * Hub 页标记：页面标题由 HubShell 按当前页签渲染为 h1，
     * Layout 不再补 sr-only h1（避免同页两个 h1）。
     */
    hub?: boolean
    /** 页面自行渲染可见 h1（如概览页页头），Layout 同样跳过 sr-only h1 */
    pageHeader?: boolean
    /** 仅管理员可访问 */
    requireAdmin?: boolean
    /**
     * 路由层 dynamic import 失败时由 router.onError 写入 LoadFailed meta，
     * LoadFailedRoute 页面读取后展示给用户。仅在 /load-failed 路由上使用。
     */
    loadError?: string
    /** 触发加载失败的来源路径（方便"重试加载"还原到原页面） */
    loadErrorFrom?: string
    /**
     * 兜底页种类，仅在 /forbidden 与 /not-found 两个顶层路由上使用：
     * 两者共用一个 RouteFallback 组件，由本字段决定文案与可用操作。
     */
    fallbackKind?: 'forbidden' | 'not-found'
  }
}

export {}