/**
 * 开发环境用：模拟多用户/角色切换（仅 VITE_USE_MOCK=true 时被加载）
 *
 * 角色/权限定义见 [../../types/roles.ts]，本文件只提供开发演示所需的用户列表和切换能力。
 * 后端 api_auth 当前是单用户架构，**此模块不会被生产包打包**：
 *   - vite.config.ts 的 manualChunks 将 /src/mock/* 集中到 'mock-data' chunk
 *   - usePermission.ts 在生产模式通过 isProductionMode 守卫，不会触发本模块的动态 import
 */
import { ROLES, type Role } from '../../types/roles'

export { ROLES }
export type { Role }
export type Permission = Role['permissions'][number]

export interface User {
  id: string
  username: string
  displayName: string
  roleId: Role['id']
  enabled: boolean
  createdAt: number
  lastLoginAt?: number
}

const users: User[] = [
  { id: 'u1', username: 'admin', displayName: '系统管理员', roleId: 'admin', enabled: true, createdAt: Date.now() - 86400000 * 30, lastLoginAt: Date.now() },
  { id: 'u2', username: 'operator1', displayName: '运维小张', roleId: 'operator', enabled: true, createdAt: Date.now() - 86400000 * 15, lastLoginAt: Date.now() - 3600000 },
  { id: 'u3', username: 'viewer1', displayName: '访客小李', roleId: 'viewer', enabled: true, createdAt: Date.now() - 86400000 * 7 },
  { id: 'u4', username: 'disabled1', displayName: '已停用账号', roleId: 'viewer', enabled: false, createdAt: Date.now() - 86400000 * 3 },
]

const currentUser: User | null = null

function delay<T>(value: T, ms = 150): Promise<T> {
  return new Promise((r) => setTimeout(() => r(value), ms))
}

export function listRoles(): Promise<Role[]> {
  return delay(Object.values(ROLES).map((r) => ({ ...r })))
}

export function listUsers(): Promise<User[]> {
  // User 字段全为原始类型，展开拷贝即可；不用 JSON.parse(JSON.stringify())
  // ——后者返回 any，会绕过 User 类型检查（漂移盲区）
  return delay(users.map((u) => ({ ...u })))
}

/** 保留以兼容开发模式下 usePermission / 老 mock 调用方 */
export function getUserPermissions(user: User): Role['permissions'] {
  return ROLES[user.roleId].permissions
}

/** 保留以兼容开发模式下 usePermission / 老 mock 调用方 */
export function hasPermission(user: User | null, perm: Role['permissions'][number]): boolean {
  if (!user) return false
  return ROLES[user.roleId].permissions.includes(perm)
}

export function getCurrentUser(): User | null {
  return currentUser
}

export function createUser(username: string, displayName: string, roleId: string): Promise<User> {
  if (users.some((u) => u.username === username)) {
    return new Promise((_, reject) => setTimeout(() => reject(new Error('用户名已存在')), 200))
  }
  const u: User = {
    id: `u${Date.now()}`,
    username,
    displayName,
    roleId: roleId as Role['id'],
    enabled: true,
    createdAt: Date.now(),
  }
  users.push(u)
  return delay({ ...u })
}

export function deleteUser(id: string): Promise<void> {
  const idx = users.findIndex((u) => u.id === id)
  if (idx >= 0) users.splice(idx, 1)
  return delay(undefined)
}

export function updateUserRole(id: string, roleId: string): Promise<void> {
  const u = users.find((x) => x.id === id)
  if (u) u.roleId = roleId as Role['id']
  return delay(undefined)
}

export function setUserEnabled(id: string, enabled: boolean): Promise<void> {
  const u = users.find((x) => x.id === id)
  if (u) u.enabled = enabled
  return delay(undefined)
}
