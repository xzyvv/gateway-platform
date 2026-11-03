import { request } from './index'

export interface LoginResponse {
  token: string
}

/** POST /api/auth/login — 用户名密码换取 token */
export function login(username: string, password: string): Promise<LoginResponse> {
  return request<LoginResponse>({
    url: '/api/auth/login',
    method: 'POST',
    data: { username, password },
  })
}

/** POST /api/auth/password — 修改密码（需已登录；服务端校验当前密码，成功后轮换 api_token） */
export function changePassword(oldPassword: string, newPassword: string): Promise<{ status: string }> {
  return request({
    url: '/api/auth/password',
    method: 'POST',
    data: { old_password: oldPassword, new_password: newPassword },
  })
}

export interface WhoAmIResponse {
  username: string
  /** 当前是否仍使用默认密码（password_hash 为空），登录后弹一次提醒 */
  using_default_password: boolean
  /** 当前配置的安全风险项（默认凭据 / 匿名只读 / 开放 CORS / 全网卡监听），空数组=无风险 */
  security_warnings: string[]
  /**
   * 调用者凭证类型：`primary` = Web 登录换取的主 token（等同管理员）；
   * `api_key` = API 密钥。旧版本网关可能不返回，按未知处理。
   */
  credential?: 'primary' | 'api_key' | 'unknown'
  /** 主 token 为 `["*"]`；API 密钥为其 scope 列表 */
  scopes?: string[]
}

/**
 * GET /api/auth/whoami — 校验当前 token 是否仍有效，并返回身份信息
 *
 * 用途：路由守卫 / 应用启动时调用，
 * 避免 localStorage 残留的过期 token 让用户看到"已登录"状态，
 * 实际每次业务请求都 401 才被发现。
 *
 * 失败（401 / 网络错）由调用方决定是否清 token 并跳 /login。
 */
export function whoami(): Promise<WhoAmIResponse> {
  return request<WhoAmIResponse>({ url: '/api/auth/whoami', method: 'GET' })
}
