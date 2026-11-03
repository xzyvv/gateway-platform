/**
 * 权限 composable
 *
 * 设计要点：
 * - 生产模式（VITE_USE_MOCK 未设或 != 'true'）：从 localStorage.gw_username 读取当前登录用户；
 *   后端没有「用户 → 角色」映射，故角色由 auth store 按 whoami 返回的
 *   `credential`/`scopes` 推导（主 token → admin，API 密钥按写 scope → operator/viewer）。
 *   该角色只用于界面显隐，真正的授权由后端 API Key scope 强制执行。
 * - 开发模式（VITE_USE_MOCK=true）：动态 import mock/system/userPermissions，支持切换 operator/viewer
 *   演示不同权限下按钮显隐。注意：mock 路径被 vite manualChunks 集中到 'mock-data' chunk，
 *   生产构建时整条动态 import 链被 tree-shake 掉，**不会进入生产包**
 *
 * #17 重构：roleId 从模块级 ref 迁移到 auth store，消除全局单例反模式。
 * 权限矩阵定义见 [../types/roles.ts]
 */
import { computed } from 'vue'
import { ROLES, roleHasPermission, type Permission, type Role } from '@/types/roles'
import { useAuthStore } from '@/stores/auth'
import { scopedGet } from '@/utils/gatewayStorage'

// 必须在本模块内直接读取编译期 env 字面量：跨模块 import 的 const（即便初值是字面量）
// 不会被 Rollup 传播到条件动态 import() 的死分支，曾导致 userPermissions 泄漏进 mock-data chunk。
// 同模块 const 经 Vite define 替换后可被可靠折叠，生产构建时第 19 行的 import() 整链消除。
const isProductionMode = import.meta.env.VITE_USE_MOCK !== 'true'

// 仅在开发模式动态加载；生产构建时该 import() 不会触发 chunk 加载
const mockModulePromise = isProductionMode ? null : import('@/mock/system/userPermissions')

/**
 * 读当前网关命名空间下的 gw_username，登录页 ([auth/Login.vue](../views/auth/Login.vue)) 已写入。
 * 走 scopedGet：多网关切换时用户名不会串台（Web 模式前缀为空，行为不变）。
 */
function readLoggedUsername(): string | null {
  return scopedGet('gw_username')
}

export function usePermission() {
  const authStore = useAuthStore()

  const user = computed(() => {
    if (isProductionMode) {
      const username = readLoggedUsername()
      return username ? { username, roleId: authStore.roleId } : null
    }
    return null // dev 模式下由 initDevState() 填充
  })

  const permissions = computed<Permission[]>(() => {
    return ROLES[authStore.roleId]?.permissions ?? []
  })

  const can = (perm: Permission) => roleHasPermission(ROLES[authStore.roleId], perm)

  /** 开发模式：切换当前角色（用于演示按钮显隐），生产模式无效 */
  function setRole(id: Role['id']) {
    if (isProductionMode) return
    authStore.roleId = id
  }

  return {
    user,
    permissions,
    can,
    setRole,
  }
}

// 开发模式初始化：动态加载 mock，读取 mock 中保存的"当前 mock 用户"角色
if (!isProductionMode) {
  void mockModulePromise!.then(({ getCurrentUser }) => {
    const u = getCurrentUser()
    const authStore = useAuthStore()
    if (u && (u.roleId === 'admin' || u.roleId === 'operator' || u.roleId === 'viewer')) {
      authStore.roleId = u.roleId
    }
  })
}