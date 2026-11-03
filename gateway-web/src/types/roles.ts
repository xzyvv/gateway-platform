/**
 * 角色与权限矩阵（共享定义）
 *
 * 设计要点：
 * - 这是唯一权威的角色/权限定义，供 usePermission 与 mock/system/userPermissions 共同引用
 * - 后端鉴权是「主 api_token（管理员等价）+ API 密钥 scope（devices/points/config 域读写）」；
 *   本文件的角色/权限只用于前端"操作按钮显隐"与"误操作拦截"，不参与后端授权判定
 * - API 密钥的写权限按路径所属域校验（`config:write` / `points:write` / `devices:write`），
 *   系统域写操作仅主 token 可执行，详见 docs/api-access.md 第 3 节
 * - 后端没有「用户 → 角色」映射：角色只由 whoami 返回的 `credential`/`scopes` 推导
 *   （主 token = admin，API 密钥按写 scope = operator/viewer），仅用于界面显隐
 * - 开发环境（VITE_USE_MOCK=true）可切换为 operator / viewer 用于演示按钮差异
 */

export type Permission =
  | 'config.read' | 'config.write' | 'config.reload'
  | 'device.read' | 'device.write'
  | 'point.read' | 'point.write'
  | 'ota.upgrade' | 'ota.rollback'
  | 'system.read' | 'user.manage'
  | 'logs.read'

export interface Role {
  id: 'admin' | 'operator' | 'viewer'
  name: string
  description: string
  permissions: Permission[]
}

export const ROLES: Record<Role['id'], Role> = {
  admin: {
    id: 'admin',
    name: '管理员',
    description: '拥有全部权限',
    permissions: [
      'config.read', 'config.write', 'config.reload',
      'device.read', 'device.write',
      'point.read', 'point.write',
      'ota.upgrade', 'ota.rollback',
      'system.read', 'user.manage',
      'logs.read',
    ],
  },
  operator: {
    id: 'operator',
    name: '运维人员',
    description: '可读写设备与点位，不可修改系统配置与用户',
    permissions: [
      'config.read',
      'device.read', 'device.write',
      'point.read', 'point.write',
      'logs.read',
    ],
  },
  viewer: {
    id: 'viewer',
    name: '只读用户',
    description: '仅可查看数据，不可执行任何写操作',
    permissions: [
      'config.read',
      'device.read',
      'point.read',
      'system.read',
      'logs.read',
    ],
  },
}

/** 判断角色是否拥有指定权限 */
export function roleHasPermission(role: Role, perm: Permission): boolean {
  return role.permissions.includes(perm)
}
