/**
 * 页面级整页导航封装（assign / reload）。
 *
 * 存在的唯一理由：可测试性。Vitest vmThreads 池的 VM 全局把
 * window.location 的 assign/reload 实现为不可重定义属性
 * （Cannot redefine property），测试无法直接 spyOn/defineProperty；
 * 经本模块间接一层后，测试用 vi.mock('@/utils/navigation') 即可断言调用。
 */
/** 整页跳转到指定 URL（触发完整资源重载，绕过 SPA 路由的 chunk 缓存） */
export function navigateTo(url: string): void {
  window.location.assign(url)
}
/** 整页刷新 */
export function reloadPage(): void {
  window.location.reload()
}
